//! Panels for a wall display: indoor air quality, weather, and departures.
//!
//! One process serves them all, one route each, so adding a panel later is a
//! route and an HTML file rather than another service to deploy. Every panel
//! is optional; a host configures only what it wants on the wall.
//!
//! Each page is static HTML baked into the binary, and fetches its own JSON
//! from `/<panel>/data`. The polling happens here, on the service's own
//! schedule, so a page that is only on screen a quarter of the time still has
//! a full history behind it, and upstream is asked once no matter how many
//! browsers are looking.

mod air;
mod config;
mod feed;
mod transit;
mod weather;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

use config::Config;

const AIR_PAGE: &str = include_str!("../pages/air.html");
const WEATHER_PAGE: &str = include_str!("../pages/weather.html");
const TRANSIT_PAGE: &str = include_str!("../pages/transit.html");
const INDEX_PAGE: &str = include_str!("../pages/index.html");

pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

struct Service {
    air: Option<Arc<air::Panel>>,
    weather: Option<Arc<feed::Feed>>,
    transit: Option<Arc<feed::Feed>>,
}

fn header(kind: &str) -> tiny_http::Header {
    tiny_http::Header::from_bytes(&b"Content-Type"[..], kind.as_bytes()).unwrap()
}

fn respond(request: tiny_http::Request, status: u16, kind: &str, body: String) {
    let response = tiny_http::Response::from_string(body)
        .with_status_code(status)
        .with_header(header(kind))
        .with_header(
            tiny_http::Header::from_bytes(&b"Cache-Control"[..], &b"no-store"[..]).unwrap(),
        );
    let _ = request.respond(response);
}

fn page(request: tiny_http::Request, body: &str) {
    respond(request, 200, "text/html; charset=utf-8", body.to_string())
}

fn data(request: tiny_http::Request, body: serde_json::Value) {
    respond(request, 200, "application/json", body.to_string())
}

fn missing(request: tiny_http::Request) {
    respond(
        request,
        404,
        "text/plain; charset=utf-8",
        "not configured\n".to_string(),
    );
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

impl Service {
    fn cached(&self, which: &Option<Arc<feed::Feed>>) -> Option<serde_json::Value> {
        let handle = which.as_ref()?;
        let body = handle.state.lock().unwrap().value.clone();
        Some(handle.envelope(body.unwrap_or(serde_json::Value::Null)))
    }

    fn handle(&self, request: tiny_http::Request) {
        let url = request.url().to_string();
        let path = url.split('?').next().unwrap_or("").to_string();
        let method = request.method().clone();
        use tiny_http::Method::{Get, Post};
        match (method, path.as_str()) {
            (Get, "/") => page(request, INDEX_PAGE),

            (Get, "/air") => page(request, AIR_PAGE),
            (Get, "/air/data") => match &self.air {
                Some(panel) => data(request, panel.data()),
                None => missing(request),
            },
            (Post, "/air/mute") => match &self.air {
                Some(panel) => {
                    let minutes = query(&url, "minutes", 30.0);
                    panel.mute(minutes);
                    respond(
                        request,
                        200,
                        "text/plain",
                        format!("muted for {minutes} min\n"),
                    )
                }
                None => missing(request),
            },
            (Post, "/air/unmute") => match &self.air {
                Some(panel) => {
                    panel.unmute();
                    respond(request, 200, "text/plain", "unmuted\n".to_string())
                }
                None => missing(request),
            },

            (Get, "/weather") => page(request, WEATHER_PAGE),
            (Get, "/weather/data") => match self.cached(&self.weather) {
                Some(body) => data(request, body),
                None => missing(request),
            },

            (Get, "/transit") => page(request, TRANSIT_PAGE),
            (Get, "/transit/data") => match self.cached(&self.transit) {
                Some(body) => data(request, body),
                None => missing(request),
            },

            (Get, "/panels") => data(
                request,
                json!({
                    "air": self.air.is_some(),
                    "weather": self.weather.is_some(),
                    "transit": self.transit.is_some(),
                }),
            ),

            _ => respond(
                request,
                404,
                "text/plain; charset=utf-8",
                "not found\n".to_string(),
            ),
        }
    }
}

fn config_path() -> PathBuf {
    if let Some(arg) = std::env::args().nth(1) {
        return PathBuf::from(arg);
    }
    match std::env::var("WALL_DASHBOARDS_CONFIG") {
        Ok(path) => PathBuf::from(path),
        Err(_) => {
            eprintln!("usage: wall-dashboards <config.json>  (or set WALL_DASHBOARDS_CONFIG)");
            std::process::exit(2);
        }
    }
}

fn main() {
    let cfg = match Config::load(&config_path()) {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(2);
        }
    };
    let _ = std::fs::create_dir_all(&cfg.state_dir);

    let air = cfg.air.map(|panel| {
        let handle = Arc::new(air::Panel::new(panel, &cfg.state_dir));
        std::thread::spawn({
            let handle = handle.clone();
            move || air::poll_forever(handle)
        });
        if !handle.cfg.alarm.device.is_empty() {
            std::thread::spawn({
                let handle = handle.clone();
                move || air::alarm_forever(handle)
            });
        }
        handle
    });

    let service = Service {
        air,
        weather: cfg.weather.map(weather::spawn),
        transit: cfg.transit.map(transit::spawn),
    };

    let address = format!("{}:{}", cfg.listen, cfg.port);
    let server = tiny_http::Server::http(&address)
        .unwrap_or_else(|e| panic!("cannot listen on {address}: {e}"));
    for request in server.incoming_requests() {
        service.handle(request);
    }
}
