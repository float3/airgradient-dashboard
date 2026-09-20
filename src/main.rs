//! Poll an AirGradient monitor's local API, serve its recent history, and alarm.
//!
//! The monitor only reports current values (GET /measures/current), so this
//! keeps the history itself: one JSON line per reading in
//! STATE_DIRECTORY/readings.jsonl, trimmed to RETENTION_HOURS. The page at /
//! draws it; /data returns it.
//!
//! Any measure can be given a safe range (LIMITS). A reading outside its range
//! for ALARM_CONSECUTIVE polls in a row raises an alarm: the page outlines that
//! chart in red, and, if ALARM_DEVICE is set, a tone pattern is sounded every
//! ALARM_REPEAT_SECONDS until the air recovers.
//!
//! The alarm is evaluated here rather than in the page on purpose. The wall
//! display rotates through several pages, so the charts are on screen roughly a
//! quarter of the time; an alarm driven by the page would stay silent the rest
//! of the time.

use std::collections::{BTreeMap, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::json;

/// The page is baked into the binary, so the service is a single artifact.
const PAGE: &str = include_str!("../index.html");

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
struct Reading {
    t: i64,
    #[serde(flatten)]
    values: BTreeMap<String, f64>,
}

#[derive(Clone, Copy, Serialize, Deserialize, Default)]
struct Limit {
    #[serde(skip_serializing_if = "Option::is_none")]
    min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max: Option<f64>,
}

struct Config {
    sensor_url: String,
    listen: String,
    port: u16,
    interval: u64,
    retention: f64,
    window: i64,
    state: PathBuf,
    limits: BTreeMap<String, Limit>,
    alarm_consecutive: u32,
    alarm_repeat: u64,
    alarm_device: String,
    /// Run through `sh -c` whenever the set of alarms changes, for pushing the
    /// news somewhere the room's own hardware cannot reach.
    alarm_command: String,
    /// [hertz, milliseconds] pairs; a frequency of 0 is a rest.
    alarm_pattern: Vec<[i64; 2]>,
    alarm_on_stale: bool,
    stale_polls: i64,
}

#[derive(Default)]
struct State {
    readings: VecDeque<Reading>,
    last_error: Option<String>,
    /// measure (or "sensor") -> "low" | "high" | "stale", once it has persisted.
    alarms: BTreeMap<String, String>,
    bad_streak: BTreeMap<String, u32>,
    good_streak: BTreeMap<String, u32>,
    muted_until: f64,
}

fn env_or(name: &str, fallback: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| fallback.to_string())
}

fn parse_or<T: std::str::FromStr>(name: &str, fallback: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(fallback)
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

impl Config {
    fn from_env() -> Self {
        let sensor = std::env::var("AIRGRADIENT_URL")
            .expect("AIRGRADIENT_URL must be set to the monitor's base URL");
        let limits: BTreeMap<String, Limit> =
            serde_json::from_str(&env_or("LIMITS", "{}")).expect("LIMITS must be a JSON object");
        let pattern: Vec<[i64; 2]> = serde_json::from_str(&env_or(
            "ALARM_PATTERN",
            "[[880,200],[0,120],[880,200],[0,120],[880,200]]",
        ))
        .expect("ALARM_PATTERN must be a JSON list of [hertz, milliseconds] pairs");
        Config {
            sensor_url: format!("{}/measures/current", sensor.trim_end_matches('/')),
            listen: env_or("LISTEN_ADDRESS", "127.0.0.1"),
            port: parse_or("PORT", 8123),
            interval: parse_or("POLL_SECONDS", 30),
            retention: parse_or::<f64>("RETENTION_HOURS", 24.0) * 3600.0,
            window: (parse_or::<f64>("WINDOW_HOURS", 3.0) * 3600.0) as i64,
            state: PathBuf::from(env_or("STATE_DIRECTORY", ".")).join("readings.jsonl"),
            limits,
            alarm_consecutive: parse_or("ALARM_CONSECUTIVE", 3),
            alarm_repeat: parse_or("ALARM_REPEAT_SECONDS", 60),
            alarm_device: env_or("ALARM_DEVICE", ""),
            alarm_command: env_or("ALARM_COMMAND", ""),
            alarm_pattern: pattern,
            alarm_on_stale: env_or("ALARM_ON_STALE", "1") == "1",
            stale_polls: parse_or("STALE_POLLS", 5),
        }
    }
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
fn settle(state: &mut State, cfg: &Config, bad: &BTreeMap<String, String>) {
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
                if *n >= cfg.alarm_consecutive {
                    state.alarms.insert(key, how.clone());
                }
            }
            None => {
                state.bad_streak.insert(key.clone(), 0);
                let n = state.good_streak.entry(key.clone()).or_insert(0);
                *n += 1;
                if *n >= cfg.alarm_consecutive {
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
fn notify(cfg: &Config, before: &BTreeMap<String, String>, after: &BTreeMap<String, String>) {
    if cfg.alarm_command.is_empty() || before == after {
        return;
    }
    let state = if after.is_empty() {
        "cleared"
    } else if before.is_empty() {
        "raised"
    } else {
        "changed"
    };
    let measures: Vec<String> = after.iter().map(|(k, v)| format!("{k}={v}")).collect();
    let text = if after.is_empty() {
        "air quality back within range".to_string()
    } else {
        after
            .iter()
            .map(|(k, v)| phrase(k, v))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let command = cfg.alarm_command.clone();
    let measures = measures.join(",");
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

fn load(cfg: &Config, state: &mut State) {
    let Ok(file) = File::open(&cfg.state) else {
        return;
    };
    let cutoff = now() - cfg.retention;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if let Ok(row) = serde_json::from_str::<Reading>(&line) {
            if row.t as f64 >= cutoff {
                state.readings.push_back(row);
            }
        }
    }
}

/// Rewrite the file without readings older than the retention window.
fn compact(cfg: &Config, state: &State) -> std::io::Result<()> {
    let tmp = cfg.state.with_extension("tmp");
    let mut handle = File::create(&tmp)?;
    for row in &state.readings {
        writeln!(handle, "{}", serde_json::to_string(row)?)?;
    }
    handle.sync_all()?;
    std::fs::rename(&tmp, &cfg.state)
}

/// No reading for stale_polls intervals.
fn is_stale(cfg: &Config, state: &State) -> bool {
    match state.readings.back() {
        None => true,
        Some(last) => now() - last.t as f64 > (cfg.stale_polls * cfg.interval as i64) as f64,
    }
}

fn fetch(cfg: &Config) -> Result<Reading, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(10))
        .build();
    // Read the body as text and parse it here rather than enabling ureq's own
    // json feature: it is the same work, one feature fewer.
    let text = agent
        .get(&cfg.sensor_url)
        .call()
        .map_err(|e| e.to_string())?
        .into_string()
        .map_err(|e| e.to_string())?;
    let body: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    Ok(pick(&body))
}

fn poll_forever(cfg: Arc<Config>, shared: Arc<Mutex<State>>) {
    let mut polls: u64 = 0;
    loop {
        match fetch(&cfg) {
            Ok(row) => {
                let mut state = shared.lock().unwrap();
                if let Ok(line) = serde_json::to_string(&row) {
                    if let Ok(mut handle) =
                        OpenOptions::new().create(true).append(true).open(&cfg.state)
                    {
                        let _ = writeln!(handle, "{line}");
                    }
                }
                state.readings.push_back(row.clone());
                let cutoff = now() - cfg.retention;
                while state.readings.front().is_some_and(|r| (r.t as f64) < cutoff) {
                    state.readings.pop_front();
                }
                polls += 1;
                if polls % 120 == 0 {
                    let _ = compact(&cfg, &state);
                }
                let verdict = judge(&row, &cfg.limits);
                let before = state.alarms.clone();
                settle(&mut state, &cfg, &verdict);
                notify(&cfg, &before, &state.alarms);
                state.last_error = None;
            }
            Err(err) => {
                // The sensor rebooting or off the network. Leave the measures
                // as they were: only the sensor is at fault, and the last good
                // reading is still what the page shows.
                let mut state = shared.lock().unwrap();
                state.last_error = Some(err);
                let mut verdict = BTreeMap::new();
                if cfg.alarm_on_stale && is_stale(&cfg, &state) {
                    verdict.insert("sensor".to_string(), "stale".to_string());
                }
                let before = state.alarms.clone();
                settle(&mut state, &cfg, &verdict);
                notify(&cfg, &before, &state.alarms);
            }
        }
        thread::sleep(Duration::from_secs(cfg.interval));
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

fn alarm_forever(cfg: Arc<Config>, shared: Arc<Mutex<State>>) {
    let mut last_sound = 0.0_f64;
    loop {
        let stamp = now();
        let ringing = {
            let state = shared.lock().unwrap();
            !state.alarms.is_empty() && stamp >= state.muted_until
        };
        if !ringing {
            last_sound = 0.0; // sound at once when it next goes off
        } else if stamp - last_sound >= cfg.alarm_repeat as f64 {
            last_sound = stamp;
            if let Err(err) = sound(&cfg.alarm_device, &cfg.alarm_pattern) {
                shared.lock().unwrap().last_error = Some(format!("alarm device: {err}"));
            }
        }
        thread::sleep(Duration::from_secs(1));
    }
}

fn query(url: &str, name: &str, fallback: f64) -> f64 {
    let Some((_, raw)) = url.split_once('?') else {
        return fallback;
    };
    for part in raw.split('&') {
        if let Some((key, value)) = part.split_once('=') {
            if key == name {
                return value.parse().unwrap_or(fallback);
            }
        }
    }
    fallback
}

fn header(kind: &str) -> tiny_http::Header {
    tiny_http::Header::from_bytes(&b"Content-Type"[..], kind.as_bytes()).unwrap()
}

fn respond(request: tiny_http::Request, status: u16, kind: &str, body: String) {
    let response = tiny_http::Response::from_string(body)
        .with_status_code(status)
        .with_header(header(kind))
        .with_header(tiny_http::Header::from_bytes(&b"Cache-Control"[..], &b"no-store"[..]).unwrap());
    let _ = request.respond(response);
}

fn serve(cfg: Arc<Config>, shared: Arc<Mutex<State>>) {
    let address = format!("{}:{}", cfg.listen, cfg.port);
    let server = tiny_http::Server::http(&address)
        .unwrap_or_else(|e| panic!("cannot listen on {address}: {e}"));
    for request in server.incoming_requests() {
        let url = request.url().to_string();
        let method = request.method().clone();
        match (method, url.split('?').next().unwrap_or("")) {
            (tiny_http::Method::Get, "/") => {
                respond(request, 200, "text/html; charset=utf-8", PAGE.to_string())
            }
            (tiny_http::Method::Get, "/data") => {
                let stamp = now() as i64;
                let state = shared.lock().unwrap();
                let recent: Vec<&Reading> = state
                    .readings
                    .iter()
                    .filter(|r| r.t >= stamp - cfg.window)
                    .collect();
                let body = json!({
                    "now": stamp,
                    "interval": cfg.interval,
                    "error": state.last_error,
                    "limits": cfg.limits,
                    "alarms": state.alarms,
                    "muted": (state.muted_until - stamp as f64).max(0.0) as i64,
                    "readings": recent,
                });
                drop(state);
                respond(request, 200, "application/json", body.to_string());
            }
            (tiny_http::Method::Post, "/mute") => {
                let minutes = query(&url, "minutes", 30.0);
                shared.lock().unwrap().muted_until = now() + minutes * 60.0;
                respond(
                    request,
                    200,
                    "text/plain",
                    format!("muted for {minutes} min\n"),
                );
            }
            (tiny_http::Method::Post, "/unmute") => {
                shared.lock().unwrap().muted_until = 0.0;
                respond(request, 200, "text/plain", "unmuted\n".to_string());
            }
            _ => respond(request, 404, "text/plain", "not found\n".to_string()),
        }
    }
}

fn main() {
    let cfg = Arc::new(Config::from_env());
    if let Some(parent) = cfg.state.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let shared = Arc::new(Mutex::new(State::default()));
    load(&cfg, &mut shared.lock().unwrap());

    thread::spawn({
        let (cfg, shared) = (cfg.clone(), shared.clone());
        move || poll_forever(cfg, shared)
    });
    if !cfg.alarm_device.is_empty() {
        thread::spawn({
            let (cfg, shared) = (cfg.clone(), shared.clone());
            move || alarm_forever(cfg, shared)
        });
    }
    serve(cfg, shared);
}
