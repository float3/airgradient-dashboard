"""Poll an AirGradient monitor's local API, serve its recent history, and alarm.

The monitor only reports current values (GET /measures/current), so this keeps
the history itself: one JSON line per reading in STATE_DIR/readings.jsonl,
trimmed to RETENTION_HOURS. The page at / draws it; /data returns it.

Any measure can be given a safe range (LIMITS). A reading outside its range for
ALARM_CONSECUTIVE polls in a row raises an alarm: the page outlines that chart
in red, and, if ALARM_DEVICE is set, a tone pattern is sounded every
ALARM_REPEAT_SECONDS until the air recovers.

The alarm is evaluated here rather than in the page on purpose. The wall display
rotates through several pages, so the chart page is on screen roughly a quarter
of the time; an alarm driven by the page would stay silent the rest of the time.
"""

import json
import os
import struct
import threading
import time
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

SENSOR_URL = os.environ["AIRGRADIENT_URL"].rstrip("/") + "/measures/current"
LISTEN = os.environ.get("LISTEN_ADDRESS", "127.0.0.1")
PORT = int(os.environ.get("PORT", "8123"))
INTERVAL = int(os.environ.get("POLL_SECONDS", "30"))
RETENTION = float(os.environ.get("RETENTION_HOURS", "24")) * 3600
WINDOW = int(float(os.environ.get("WINDOW_HOURS", "3")) * 3600)
STATE = Path(os.environ.get("STATE_DIRECTORY", ".")) / "readings.jsonl"
PAGE = Path(__file__).with_name("index.html").read_bytes()

# {"rco2": {"max": 1000}, "atmp": {"min": 18, "max": 27}, ...}; a measure with
# no entry, or with only one side set, is simply not checked on that side.
LIMITS = json.loads(os.environ.get("LIMITS", "{}"))
# Number of consecutive readings outside the range before the alarm is raised,
# and inside it again before it clears. Keeps a single spike quiet.
ALARM_CONSECUTIVE = int(os.environ.get("ALARM_CONSECUTIVE", "3"))
ALARM_REPEAT = int(os.environ.get("ALARM_REPEAT_SECONDS", "60"))
# evdev node of the PC speaker, e.g.
# /dev/input/by-path/platform-pcspkr-event-spkr. Empty disables the sound.
ALARM_DEVICE = os.environ.get("ALARM_DEVICE", "")
# [[hz, ms], ...]; a frequency of 0 is a rest.
ALARM_PATTERN = json.loads(
    os.environ.get("ALARM_PATTERN", "[[880,200],[0,120],[880,200],[0,120],[880,200]]")
)
# A sensor that stopped answering is its own alarm: the numbers left on screen
# are not the room any more.
ALARM_ON_STALE = os.environ.get("ALARM_ON_STALE", "1") == "1"
STALE_POLLS = int(os.environ.get("STALE_POLLS", "5"))

EV_SND, SND_TONE = 0x12, 0x02
EVENT = struct.Struct("@qqHHi")

# Measures kept, by API field. Where the firmware also reports a compensated
# value (corrected for the sensor's own heat and humidity), that one is used,
# as AirGradient's own dashboard does.
FIELDS = {
    "pm01": ["pm01"],
    "pm02": ["pm02Compensated", "pm02"],
    "pm10": ["pm10"],
    "rco2": ["rco2"],
    "tvoc": ["tvocIndex"],
    "nox": ["noxIndex"],
    "atmp": ["atmpCompensated", "atmp"],
    "rhum": ["rhumCompensated", "rhum"],
}

lock = threading.Lock()
readings: list[dict] = []
last_error: str | None = None
# measure (or "sensor") -> "low" | "high" | "stale", once it has persisted.
alarms: dict[str, str] = {}
bad_streak: dict[str, int] = {}
good_streak: dict[str, int] = {}
muted_until = 0.0


def pick(raw: dict) -> dict:
    out = {"t": int(time.time())}
    for key, candidates in FIELDS.items():
        for name in candidates:
            value = raw.get(name)
            if isinstance(value, (int, float)):
                out[key] = round(float(value), 2)
                break
    return out


def judge(row: dict) -> dict[str, str]:
    """Which measures in this reading sit outside their configured range."""
    out = {}
    for key, limit in LIMITS.items():
        value = row.get(key)
        if not isinstance(value, (int, float)):
            continue
        low, high = limit.get("min"), limit.get("max")
        if low is not None and value < low:
            out[key] = "low"
        elif high is not None and value > high:
            out[key] = "high"
    return out


def settle(bad: dict[str, str]) -> None:
    """Fold one verdict into the alarm state, with hysteresis both ways.

    The caller holds the lock.
    """
    for key in list(LIMITS) + ["sensor"]:
        state = bad.get(key)
        if state:
            good_streak[key] = 0
            bad_streak[key] = bad_streak.get(key, 0) + 1
            if bad_streak[key] >= ALARM_CONSECUTIVE:
                alarms[key] = state
        else:
            bad_streak[key] = 0
            good_streak[key] = good_streak.get(key, 0) + 1
            if good_streak[key] >= ALARM_CONSECUTIVE:
                alarms.pop(key, None)


def load() -> None:
    cutoff = time.time() - RETENTION
    if not STATE.exists():
        return
    with STATE.open(encoding="utf-8") as handle:
        for line in handle:
            try:
                row = json.loads(line)
            except ValueError:
                continue
            if row.get("t", 0) >= cutoff:
                readings.append(row)


def compact() -> None:
    """Rewrite the file without readings older than the retention window."""
    tmp = STATE.with_suffix(".tmp")
    with tmp.open("w", encoding="utf-8") as handle:
        for row in readings:
            handle.write(json.dumps(row) + "\n")
    tmp.replace(STATE)


def is_stale() -> bool:
    """No reading for STALE_POLLS intervals. The caller holds the lock."""
    if not readings:
        return True
    return time.time() - readings[-1]["t"] > STALE_POLLS * INTERVAL


def poll_forever() -> None:
    global last_error
    polls = 0
    while True:
        try:
            with urllib.request.urlopen(SENSOR_URL, timeout=10) as response:
                row = pick(json.load(response))
            with lock:
                readings.append(row)
                with STATE.open("a", encoding="utf-8") as handle:
                    handle.write(json.dumps(row) + "\n")
                cutoff = time.time() - RETENTION
                while readings and readings[0]["t"] < cutoff:
                    readings.pop(0)
                polls += 1
                if polls % 120 == 0:
                    compact()
                settle(judge(row))
            last_error = None
        except Exception as exc:  # the sensor rebooting or off the network
            last_error = f"{type(exc).__name__}: {exc}"
            with lock:
                # Leave the measures as they were: only the sensor is at fault,
                # and the last good reading is still what the page shows.
                stale = ALARM_ON_STALE and is_stale()
                settle({"sensor": "stale"} if stale else {})
        time.sleep(INTERVAL)


def sound(pattern) -> None:
    """Play one pattern on the PC speaker, leaving it silent afterwards."""
    handle = open(ALARM_DEVICE, "wb", buffering=0)
    try:
        for hz, ms in pattern:
            handle.write(EVENT.pack(0, 0, EV_SND, SND_TONE, int(hz)))
            time.sleep(ms / 1000)
    finally:
        try:
            handle.write(EVENT.pack(0, 0, EV_SND, SND_TONE, 0))
        finally:
            handle.close()


def alarm_forever() -> None:
    global last_error
    last_sound = 0.0
    while True:
        now = time.time()
        with lock:
            ringing = bool(alarms) and now >= muted_until
        if not ringing:
            last_sound = 0.0  # sound at once when it next goes off
        elif now - last_sound >= ALARM_REPEAT:
            last_sound = now
            try:
                sound(ALARM_PATTERN)
            except OSError as exc:
                last_error = f"alarm device: {exc}"
        time.sleep(1)


def query(path: str, name: str, fallback: float) -> float:
    _, _, raw = path.partition("?")
    for part in raw.split("&"):
        key, _, value = part.partition("=")
        if key == name:
            try:
                return float(value)
            except ValueError:
                return fallback
    return fallback


class Handler(BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        if self.path == "/":
            self.reply(200, "text/html; charset=utf-8", PAGE)
        elif self.path.startswith("/data"):
            now = int(time.time())
            with lock:
                body = {
                    "now": now,
                    "interval": INTERVAL,
                    "error": last_error,
                    "limits": LIMITS,
                    "alarms": dict(alarms),
                    "muted": max(0, int(muted_until - now)),
                    "readings": [r for r in readings if r["t"] >= now - WINDOW],
                }
            self.reply(200, "application/json", json.dumps(body).encode())
        else:
            self.reply(404, "text/plain", b"not found")

    def do_POST(self) -> None:
        global muted_until
        if self.path.startswith("/mute"):
            minutes = query(self.path, "minutes", 30.0)
            with lock:
                muted_until = time.time() + minutes * 60
            self.reply(200, "text/plain", f"muted for {minutes:g} min\n".encode())
        elif self.path.startswith("/unmute"):
            with lock:
                muted_until = 0.0
            self.reply(200, "text/plain", b"unmuted\n")
        else:
            self.reply(404, "text/plain", b"not found")

    def reply(self, status: int, kind: str, body: bytes) -> None:
        self.send_response(status)
        self.send_header("Content-Type", kind)
        self.send_header("Cache-Control", "no-store")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args) -> None:
        pass


if __name__ == "__main__":
    STATE.parent.mkdir(parents=True, exist_ok=True)
    load()
    threading.Thread(target=poll_forever, daemon=True).start()
    if ALARM_DEVICE:
        threading.Thread(target=alarm_forever, daemon=True).start()
    ThreadingHTTPServer((LISTEN, PORT), Handler).serve_forever()
