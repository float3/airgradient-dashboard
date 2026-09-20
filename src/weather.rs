//! The weather panel, from Open-Meteo.
//!
//! Open-Meteo needs no key and no account, and for this part of the world it
//! serves DWD's ICON model. The coordinates come from the config file, never
//! from this repository.

use std::sync::Arc;

use crate::config::Weather;
use crate::feed::{self, Feed};

const CURRENT: &str = "temperature_2m,apparent_temperature,relative_humidity_2m,\
                       precipitation,weather_code,wind_speed_10m,wind_direction_10m,is_day";
const HOURLY: &str = "temperature_2m,precipitation_probability,precipitation,weather_code";
const DAILY: &str = "weather_code,temperature_2m_max,temperature_2m_min,sunrise,sunset,\
                     precipitation_sum,precipitation_probability_max,wind_speed_10m_max,\
                     uv_index_max";

pub fn url(cfg: &Weather) -> String {
    format!(
        "https://api.open-meteo.com/v1/forecast\
         ?latitude={lat}&longitude={lon}&timezone={tz}\
         &current={current}&hourly={hourly}&daily={daily}\
         &forecast_days={days}",
        lat = cfg.latitude,
        lon = cfg.longitude,
        tz = urlencode(&cfg.timezone),
        current = CURRENT,
        hourly = HOURLY,
        daily = DAILY,
        days = cfg.forecast_days,
    )
}

/// Only the few characters a timezone name can contain need escaping.
fn urlencode(raw: &str) -> String {
    raw.chars()
        .map(|c| match c {
            '/' => "%2F".to_string(),
            '+' => "%2B".to_string(),
            ' ' => "%20".to_string(),
            other => other.to_string(),
        })
        .collect()
}

pub fn spawn(cfg: Weather) -> Arc<Feed> {
    let handle = Arc::new(Feed::new(cfg.poll_seconds));
    let feed = handle.clone();
    std::thread::spawn(move || {
        // Open-Meteo's shape is already what the page wants, so nothing is
        // reshaped here; the page slices the hourly arrays it needs.
        feed::poll_forever(feed, move || url(&cfg), |body| body);
    });
    handle
}
