//! The departures panel, from MVG's own endpoint.
//!
//! This is the API the mvg.de site calls, not a documented product, so it is
//! treated as something that can change under us: every field is optional on
//! the way in, the last good answer is kept when a fetch fails, and the page
//! is built to show what it has rather than to assume.
//!
//! Which stops, and how far away they are on foot, come from the config file.

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde_json::json;

use crate::config::{Stop, Transit};
use crate::feed::{self, Feed};
use crate::now;

fn url(stop: &Stop) -> String {
    format!(
        "https://www.mvg.de/api/bgw-pt/v3/departures?globalId={id}&limit={limit}",
        id = stop.global_id,
        limit = stop.limit,
    )
}

/// Reduce one departure to what a person glancing at a wall needs.
fn departure(raw: &serde_json::Value) -> serde_json::Value {
    let planned = raw.get("plannedDepartureTime").and_then(|v| v.as_i64());
    let real = raw.get("realtimeDepartureTime").and_then(|v| v.as_i64());
    // MVG reports both times; the difference is the delay, and showing it
    // matters more than showing either time on its own.
    let delay = match (planned, real) {
        (Some(p), Some(r)) => Some((r - p) / 60000),
        _ => None,
    };
    json!({
        "line": raw.get("label"),
        "destination": raw.get("destination"),
        "type": raw.get("transportType"),
        "at": real.or(planned),
        "delay": delay,
        "cancelled": raw.get("cancelled").and_then(|v| v.as_bool()).unwrap_or(false),
        "platform": raw.get("platform"),
        "occupancy": raw.get("occupancy"),
    })
}

pub fn spawn(cfg: Transit) -> Arc<Feed> {
    let handle = Arc::new(Feed::new(cfg.poll_seconds));
    let feed = handle.clone();
    thread::spawn(move || poll_forever(feed, cfg));
    handle
}

fn poll_forever(feed: Arc<Feed>, cfg: Transit) {
    loop {
        let mut stops = Vec::new();
        let mut failures = Vec::new();
        for stop in &cfg.stops {
            match feed::get_json(&url(stop)) {
                Ok(body) => {
                    let rows: Vec<serde_json::Value> = body
                        .as_array()
                        .map(|list| list.iter().map(departure).collect())
                        .unwrap_or_default();
                    stops.push(json!({
                        "name": stop.name,
                        "walkMinutes": stop.walk_minutes,
                        "departures": rows,
                    }));
                }
                Err(err) => failures.push(format!("{}: {err}", stop.name)),
            }
        }
        let mut state = feed.state.lock().unwrap();
        // A stop that failed keeps whatever it had; replacing the whole answer
        // with a partial one would blank a working board over one bad fetch.
        if !stops.is_empty() {
            state.value = Some(json!({"stops": stops}));
            state.fetched = now();
        }
        state.last_error = if failures.is_empty() {
            None
        } else {
            Some(failures.join("; "))
        };
        drop(state);
        thread::sleep(Duration::from_secs(cfg.poll_seconds));
    }
}
