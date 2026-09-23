//! A cached upstream feed.
//!
//! Weather and departures are not ours to compute: somebody else's API has
//! the answer, and the only jobs here are to ask at a sane rate, to keep the
//! last good answer when the network hiccups, and to hand the page something
//! that is already shaped. One wall display should not turn into a hundred
//! requests an hour just because four pages are rotating past it.

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::json;

use crate::now;

pub fn get_json(url: &str) -> Result<serde_json::Value, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .into();
    // Read the body as text and parse it here rather than enabling ureq's own
    // json feature: it is the same work, one feature fewer.
    let text = agent
        .get(url)
        .header("User-Agent", "wall-dashboards")
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

#[derive(Default)]
pub struct Cached {
    /// The last answer that parsed, however old.
    pub value: Option<serde_json::Value>,
    /// When it arrived.
    pub fetched: f64,
    pub last_error: Option<String>,
}

pub struct Feed {
    pub state: Mutex<Cached>,
    pub interval: u64,
}

impl Feed {
    pub fn new(interval: u64) -> Feed {
        Feed {
            state: Mutex::new(Cached::default()),
            interval,
        }
    }

    /// The cached body, with enough around it for the page to say how stale
    /// what it is drawing has become.
    pub fn envelope(&self, body: serde_json::Value) -> serde_json::Value {
        let state = self.state.lock().unwrap();
        json!({
            "now": now() as i64,
            "interval": self.interval,
            "fetched": state.fetched as i64,
            "age": (now() - state.fetched).max(0.0) as i64,
            "error": state.last_error,
            "data": body,
        })
    }
}

/// Refetch `url` forever, keeping the last good answer on failure. `shape` is
/// applied once per fetch rather than once per request, since the page asks
/// far more often than upstream changes.
pub fn poll_forever(
    feed: Arc<Feed>,
    url: impl Fn() -> String + Send + 'static,
    shape: impl Fn(serde_json::Value) -> serde_json::Value + Send + 'static,
) {
    loop {
        match get_json(&url()) {
            Ok(body) => {
                let mut state = feed.state.lock().unwrap();
                state.value = Some(shape(body));
                state.fetched = now();
                state.last_error = None;
            }
            Err(err) => {
                let mut state = feed.state.lock().unwrap();
                state.last_error = Some(err);
            }
        }
        thread::sleep(Duration::from_secs(feed.interval));
    }
}
