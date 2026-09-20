//! The air-quality panel: poll an AirGradient monitor, keep its history, and
//! raise an alarm when a measure leaves its configured range.
//!
//! The monitor only reports current values (GET /measures/current), so the
//! history is kept here: one JSON line per reading in readings.jsonl, trimmed
//! to the retention window.
//!
//! Ranges are judged here rather than in the page on purpose. A wall display
//! rotates through several pages, so any one panel is on screen a fraction of
//! the time; an alarm driven by the browser would stay silent the rest of it.

use std::collections::{BTreeMap, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::config::{Air, Limit};
use crate::feed;
use crate::now;

/// Measures kept, by API field. Where the firmware also reports a compensated
/// value (corrected for the sensor's own heat and humidity), that one is used,
/// as AirGradient's own dashboard does.
const FIELDS: &[(&str, &[&str])] = &[
    ("pm01", &["pm01"]),
    ("pm02", &["pm02Compensated", "pm02"]),
    ("pm10", &["pm10"]),
    ("rco2", &["rco2"]),
    ("tvoc", &["tvocIndex"]),
    ("nox", &["noxIndex"]),
    ("atmp", &["atmpCompensated", "atmp"]),
    ("rhum", &["rhumCompensated", "rhum"]),
];

#[derive(Clone, Serialize, Deserialize)]
pub struct Reading {
    pub t: i64,
    #[serde(flatten)]
    pub values: BTreeMap<String, f64>,
}

#[derive(Default)]
pub struct State {
    pub readings: VecDeque<Reading>,
    pub last_error: Option<String>,
    /// measure (or "sensor") -> "low" | "high" | "stale", once it has persisted.
    pub alarms: BTreeMap<String, String>,
    bad_streak: BTreeMap<String, u32>,
    good_streak: BTreeMap<String, u32>,
    pub muted_until: f64,
}

pub struct Panel {
    pub cfg: Air,
    pub state: Mutex<State>,
    pub file: PathBuf,
}

/// Reduce one sensor payload to the measures worth keeping.
fn pick(raw: &serde_json::Value) -> Reading {
    let mut values = BTreeMap::new();
    for (key, candidates) in FIELDS {
        for name in *candidates {
            if let Some(v) = raw.get(name).and_then(serde_json::Value::as_f64) {
                values.insert(key.to_string(), (v * 100.0).round() / 100.0);
                break;
            }
        }
    }
    Reading {
        t: now() as i64,
        values,
    }
}

/// Which measures in this reading sit outside their configured range.
fn judge(row: &Reading, limits: &BTreeMap<String, Limit>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (key, limit) in limits {
        let Some(&value) = row.values.get(key) else {
            continue;
        };
        if limit.min.is_some_and(|low| value < low) {
            out.insert(key.clone(), "low".to_string());
        } else if limit.max.is_some_and(|high| value > high) {
            out.insert(key.clone(), "high".to_string());
        }
    }
    out
}

/// Fold one verdict into the alarm state, with hysteresis both ways.
fn settle(state: &mut State, cfg: &Air, bad: &BTreeMap<String, String>) {
    let keys: Vec<String> = cfg
        .limits
        .keys()
        .cloned()
        .chain(std::iter::once("sensor".to_string()))
        .collect();
    for key in keys {
        match bad.get(&key) {
            Some(how) => {
                state.good_streak.insert(key.clone(), 0);
                let n = state.bad_streak.entry(key.clone()).or_insert(0);
                *n += 1;
                if *n >= cfg.alarm.consecutive {
                    state.alarms.insert(key, how.clone());
                }
            }
            None => {
                state.bad_streak.insert(key.clone(), 0);
                let n = state.good_streak.entry(key.clone()).or_insert(0);
                *n += 1;
                if *n >= cfg.alarm.consecutive {
                    state.alarms.remove(&key);
                }
            }
        }
    }
}

/// Put one measure's verdict into words a person reading a phone will follow.
fn phrase(key: &str, how: &str) -> String {
    if how == "stale" {
        "sensor not responding".to_string()
    } else {
        format!("{key} too {how}")
    }
}

/// Run the hook when, and only when, the set of alarms actually changes. A
/// repeat every minute is right for a noise in the room and wrong for a push
/// notification, so this fires on transitions alone.
fn notify(cfg: &Air, before: &BTreeMap<String, String>, after: &BTreeMap<String, String>) {
    if cfg.alarm.command.is_empty() || before == after {
        return;
    }
    let state = if after.is_empty() {
        "cleared"
    } else if before.is_empty() {
        "raised"
    } else {
        "changed"
    };
    let measures = after
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(",");
    let text = if after.is_empty() {
        "air quality back within range".to_string()
    } else {
        after
            .iter()
            .map(|(k, v)| phrase(k, v))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let command = cfg.alarm.command.clone();
    thread::spawn(move || {
        let _ = std::process::Command::new("sh")
            .arg("-c")
            .arg(&command)
            .env("ALARM_STATE", state)
            .env("ALARM_MEASURES", measures)
            .env("ALARM_TEXT", text)
            .status();
    });
}

impl Panel {
    pub fn new(cfg: Air, state_dir: &std::path::Path) -> Panel {
        let file = state_dir.join("readings.jsonl");
        let mut state = State::default();
        let cutoff = now() - cfg.retention_hours * 3600.0;
        if let Ok(handle) = File::open(&file) {
            for line in BufReader::new(handle).lines().map_while(Result::ok) {
                if let Ok(row) = serde_json::from_str::<Reading>(&line) {
                    if row.t as f64 >= cutoff {
                        state.readings.push_back(row);
                    }
                }
            }
        }
        Panel {
            cfg,
            state: Mutex::new(state),
            file,
        }
    }

    /// No reading for stale_polls intervals.
    fn is_stale(&self, state: &State) -> bool {
        match state.readings.back() {
            None => true,
            Some(last) => {
                now() - last.t as f64
                    > (self.cfg.alarm.stale_polls * self.cfg.poll_seconds as i64) as f64
            }
        }
    }

    /// Rewrite the file without readings older than the retention window.
    fn compact(&self, state: &State) -> std::io::Result<()> {
        let tmp = self.file.with_extension("tmp");
        let mut handle = File::create(&tmp)?;
        for row in &state.readings {
            writeln!(handle, "{}", serde_json::to_string(row)?)?;
        }
        handle.sync_all()?;
        std::fs::rename(&tmp, &self.file)
    }

    pub fn data(&self) -> serde_json::Value {
        let stamp = now() as i64;
        let window = (self.cfg.window_hours * 3600.0) as i64;
        let state = self.state.lock().unwrap();
        let recent: Vec<&Reading> = state
            .readings
            .iter()
            .filter(|r| r.t >= stamp - window)
            .collect();
        json!({
            "now": stamp,
            "interval": self.cfg.poll_seconds,
            "error": state.last_error,
            "limits": self.cfg.limits,
            "alarms": state.alarms,
            "muted": (state.muted_until - stamp as f64).max(0.0) as i64,
            "readings": recent,
        })
    }

    pub fn mute(&self, minutes: f64) {
        self.state.lock().unwrap().muted_until = now() + minutes * 60.0;
    }

    pub fn unmute(&self) {
        self.state.lock().unwrap().muted_until = 0.0;
    }
}

pub fn poll_forever(panel: Arc<Panel>) {
    let url = format!(
        "{}/measures/current",
        panel.cfg.sensor_url.trim_end_matches('/')
    );
    let mut polls: u64 = 0;
    loop {
        match feed::get_json(&url) {
            Ok(body) => {
                let row = pick(&body);
                let mut state = panel.state.lock().unwrap();
                if let Ok(line) = serde_json::to_string(&row) {
                    if let Ok(mut handle) = OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&panel.file)
                    {
                        let _ = writeln!(handle, "{line}");
                    }
                }
                state.readings.push_back(row.clone());
                let cutoff = now() - panel.cfg.retention_hours * 3600.0;
                while state.readings.front().is_some_and(|r| (r.t as f64) < cutoff) {
                    state.readings.pop_front();
                }
                polls += 1;
                if polls % 120 == 0 {
                    let _ = panel.compact(&state);
                }
                let verdict = judge(&row, &panel.cfg.limits);
                let before = state.alarms.clone();
                settle(&mut state, &panel.cfg, &verdict);
                notify(&panel.cfg, &before, &state.alarms);
                state.last_error = None;
            }
            Err(err) => {
                // The sensor rebooting or off the network. Leave the measures
                // as they were: only the sensor is at fault, and the last good
                // reading is still what the page shows.
                let mut state = panel.state.lock().unwrap();
                state.last_error = Some(err);
                let mut verdict = BTreeMap::new();
                if panel.cfg.alarm.on_stale && panel.is_stale(&state) {
                    verdict.insert("sensor".to_string(), "stale".to_string());
                }
                let before = state.alarms.clone();
                settle(&mut state, &panel.cfg, &verdict);
                notify(&panel.cfg, &before, &state.alarms);
            }
        }
        thread::sleep(Duration::from_secs(panel.cfg.poll_seconds));
    }
}

const EV_SND: u16 = 0x12;
const SND_TONE: u16 = 0x02;

/// One `struct input_event` with a zeroed timestamp: 64-bit seconds and
/// microseconds, then type, code and value.
fn input_event(code: u16, value: i32) -> [u8; 24] {
    let mut bytes = [0u8; 24];
    bytes[16..18].copy_from_slice(&EV_SND.to_ne_bytes());
    bytes[18..20].copy_from_slice(&code.to_ne_bytes());
    bytes[20..24].copy_from_slice(&value.to_ne_bytes());
    bytes
}

/// Play one pattern on the PC speaker, leaving it silent afterwards.
fn sound(device: &str, pattern: &[[i64; 2]]) -> std::io::Result<()> {
    let mut handle = OpenOptions::new().write(true).open(device)?;
    let result = (|| -> std::io::Result<()> {
        for [hz, ms] in pattern {
            handle.write_all(&input_event(SND_TONE, *hz as i32))?;
            thread::sleep(Duration::from_millis(*ms as u64));
        }
        Ok(())
    })();
    handle.write_all(&input_event(SND_TONE, 0))?;
    result
}

pub fn alarm_forever(panel: Arc<Panel>) {
    let mut last_sound = 0.0_f64;
    loop {
        let stamp = now();
        let ringing = {
            let state = panel.state.lock().unwrap();
            !state.alarms.is_empty() && stamp >= state.muted_until
        };
        if !ringing {
            last_sound = 0.0; // sound at once when it next goes off
        } else if stamp - last_sound >= panel.cfg.alarm.repeat_seconds as f64 {
            last_sound = stamp;
            if let Err(err) = sound(&panel.cfg.alarm.device, &panel.cfg.alarm.pattern) {
                panel.state.lock().unwrap().last_error = Some(format!("alarm device: {err}"));
            }
        }
        thread::sleep(Duration::from_secs(1));
    }
}
