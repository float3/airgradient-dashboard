//! The whole service is configured by one JSON file, named on the command
//! line or in `WALL_DASHBOARDS_CONFIG`. A panel that is absent is not polled
//! and not served, so a host takes only the panels it wants.
//!
//! Anything that says where the machine is -- coordinates, stop ids, a sensor
//! on the LAN -- belongs in that file, which is written by whoever deploys
//! this. Nothing location-bearing is baked into this repository.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

fn default_listen() -> String {
    "127.0.0.1".to_string()
}

fn default_port() -> u16 {
    8123
}

fn default_state() -> PathBuf {
    PathBuf::from(".")
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(default = "default_listen")]
    pub listen: String,
    #[serde(default = "default_port")]
    pub port: u16,
    /// Where history that must survive a restart is kept.
    #[serde(default = "default_state")]
    pub state_dir: PathBuf,
    #[serde(default)]
    pub air: Option<Air>,
    #[serde(default)]
    pub weather: Option<Weather>,
    #[serde(default)]
    pub transit: Option<Transit>,
}

#[derive(Clone, Copy, Serialize, Deserialize, Default)]
pub struct Limit {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Air {
    /// Base URL of the monitor's local API; it serves /measures/current.
    pub sensor_url: String,
    #[serde(default = "Air::default_poll")]
    pub poll_seconds: u64,
    #[serde(default = "Air::default_retention")]
    pub retention_hours: f64,
    #[serde(default = "Air::default_window")]
    pub window_hours: f64,
    #[serde(default)]
    pub limits: BTreeMap<String, Limit>,
    #[serde(default)]
    pub alarm: Alarm,
}

impl Air {
    fn default_poll() -> u64 {
        30
    }
    fn default_retention() -> f64 {
        24.0
    }
    fn default_window() -> f64 {
        3.0
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Alarm {
    /// Readings in a row outside a range before the alarm is raised, and
    /// inside it again before it clears. Keeps one spike from crying wolf.
    #[serde(default = "Alarm::default_consecutive")]
    pub consecutive: u32,
    #[serde(default = "Alarm::default_repeat")]
    pub repeat_seconds: u64,
    /// evdev node that accepts EV_SND tones, usually the PC speaker. Empty
    /// means this service makes no sound itself.
    #[serde(default)]
    pub device: String,
    /// [hertz, milliseconds] pairs; a frequency of 0 is a rest.
    #[serde(default = "Alarm::default_pattern")]
    pub pattern: Vec<[i64; 2]>,
    /// Run through `sh -c` whenever the set of alarms changes.
    #[serde(default)]
    pub command: String,
    #[serde(default = "Alarm::yes")]
    pub on_stale: bool,
    #[serde(default = "Alarm::default_stale_polls")]
    pub stale_polls: i64,
}

impl Alarm {
    fn default_consecutive() -> u32 {
        3
    }
    fn default_repeat() -> u64 {
        60
    }
    fn default_stale_polls() -> i64 {
        5
    }
    fn yes() -> bool {
        true
    }
    fn default_pattern() -> Vec<[i64; 2]> {
        vec![[880, 200], [0, 120], [880, 200], [0, 120], [880, 200]]
    }
}

impl Default for Alarm {
    fn default() -> Self {
        Alarm {
            consecutive: Alarm::default_consecutive(),
            repeat_seconds: Alarm::default_repeat(),
            device: String::new(),
            pattern: Alarm::default_pattern(),
            command: String::new(),
            on_stale: true,
            stale_polls: Alarm::default_stale_polls(),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Weather {
    pub latitude: f64,
    pub longitude: f64,
    #[serde(default = "Weather::default_timezone")]
    pub timezone: String,
    /// Open-Meteo updates its models hourly, so a quarter hour is plenty and
    /// stays well inside the free tier.
    #[serde(default = "Weather::default_poll")]
    pub poll_seconds: u64,
    /// How far the low-detail outlook strip reaches.
    #[serde(default = "Weather::default_days")]
    pub forecast_days: u8,
}

impl Weather {
    fn default_timezone() -> String {
        "auto".to_string()
    }
    fn default_poll() -> u64 {
        900
    }
    fn default_days() -> u8 {
        10
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transit {
    pub stops: Vec<Stop>,
    #[serde(default = "Transit::default_poll")]
    pub poll_seconds: u64,
}

impl Transit {
    fn default_poll() -> u64 {
        30
    }
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stop {
    /// MVG's identifier, e.g. the one behind /api/bgw-pt/v3/locations?query=.
    pub global_id: String,
    /// What to call it on screen, which need not be MVG's own name.
    pub name: String,
    #[serde(default = "Stop::default_limit")]
    pub limit: u32,
    /// Walking minutes to the stop: departures sooner than this are shown as
    /// already unreachable rather than as options.
    #[serde(default)]
    pub walk_minutes: i64,
}

impl Stop {
    fn default_limit() -> u32 {
        8
    }
}

impl Config {
    pub fn load(path: &std::path::Path) -> Result<Config, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read config {}: {e}", path.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("cannot parse {}: {e}", path.display()))
    }
}
