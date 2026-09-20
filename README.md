# airgradient-dashboard

A small local dashboard for an [AirGradient](https://www.airgradient.com/)
monitor: it polls the device's own API on your network, keeps the history
itself, draws it, and beeps the machine's PC speaker when a measure leaves the
range you set.

It exists because AirGradient's cloud dashboard logs you out, and a wall display
cannot log itself back in. Nothing here talks to AirGradient's servers.

## What it does

- Polls `GET /measures/current` on the monitor every 30 s.
- Keeps 24 h of readings as JSON lines on disk, and draws the last 3 h as eight
  charts, each on its own Y scale.
- Raises an alarm when any measure sits outside its configured range for
  several readings running: the chart gets a red outline and a red reading, the
  header says which measure and which way, and the PC speaker beeps once a
  minute until the air recovers.
- Treats a sensor that stopped answering as an alarm of its own. A frozen chart
  otherwise looks exactly like calm air.

The ranges are checked in the service, not in the page, on purpose. A wall
display usually rotates through several pages, so the charts are on screen only
part of the time; an alarm driven by the browser would stay quiet the rest of
it.

### How the alarm reaches you

The service itself can do two things, and neither assumes a sound card:

- `alarm.device` writes `EV_SND` tones to an evdev node, normally the PC
  speaker.
- `alarm.command` runs a shell line whenever the set of alarms *changes*, with
  `ALARM_STATE`, `ALARM_MEASURES` and `ALARM_TEXT` in the environment. It fires
  on transitions only, never on the repeat, which is what a push notification
  wants.

Anything that needs the machine's sound hardware belongs in a user service that
polls `/data` instead: this one runs as a `DynamicUser` with no seat and no
access to anyone's audio session.

**Do not assume the PC speaker works.** A machine that beeps at power-on often
cannot be made to beep from Linux: on many small-form-factor boards the POST
beep comes from the embedded controller, and the PIT speaker line that
`pcspkr` drives is connected to nothing. The driver will accept your tones and
report success while making no sound at all. Test with a long continuous tone
before you rely on it, and check that the BIOS pin config is not merely
claiming an internal speaker that was never fitted.

## Use it

```nix
{
  inputs.airgradient-dashboard.url = "github:float3/airgradient-dashboard";

  # in your host's modules:
  imports = [inputs.airgradient-dashboard.nixosModules.default];

  services.airgradient-dashboard = {
    enable = true;
    sensorUrl = "http://192.168.1.80";
    limits = {
      rco2.max = 1000;
      atmp = {
        min = 18;
        max = 27;
      };
    };
    alarm.command = ''
      curl -fsS -H "Title: Air quality" -d "$ALARM_TEXT" https://ntfy.sh/my-topic
    '';
  };
}
```

Prefer a plain address over the monitor's `.local` name where you can: an mDNS
name makes the dashboard depend on avahi being healthy as well as on the sensor,
and avahi is the more fragile of the two.

Every option is documented in [`module.nix`](module.nix). The defaults alarm on
CO₂ above 1000 ppm, temperature outside 18–27 °C, humidity outside 30–60 %,
PM2.5 above 15 µg/m³, PM10 above 45 µg/m³, TVOC index above 250 and NOx index
above 50.

Without flakes, `module.nix` is an ordinary NixOS module and `package.nix` an
ordinary `callPackage` file.

## Silence it

```sh
curl -X POST 'http://127.0.0.1:8123/mute?minutes=30'
curl -X POST http://127.0.0.1:8123/unmute
```

Muting stops the beeping only. The red outline stays, because the air is still
bad.

## Notes on the numbers

- **An AirGradient ONE has no oxygen sensor.** CO₂ is the only stand-in for a
  room going stuffy, which is why it carries the default range it does.
- **PM readings of exactly 0 are usually real.** The PMS5003 quantises mass to
  1 µg/m³, so in clean indoor air PM1 and PM10 sit at 0 and the particle counts
  (`pm003Count` and friends, which this dashboard does not chart) carry what
  signal there is.
- **PM2.5 jumps between 0 and about 1.6.** The dashboard plots
  `pm02Compensated`, the humidity-corrected value, as AirGradient's own
  dashboard does. The firmware special-cases a raw 0 to 0 but maps a raw 1 to
  roughly 1.6, so the compensated trace has a gap it can never land in. It looks
  like noise and is not.

## Run it by hand

```sh
AIRGRADIENT_URL=http://192.168.1.80 STATE_DIRECTORY=/tmp/ag cargo run --release
```

Then open <http://127.0.0.1:8123>. Every option is an environment variable;
`module.nix` is only a typed front end to them.

The server is Rust, and the page it serves is baked into the binary, so the
service is a single artifact with nothing beside it to install. The page itself
is plain HTML with hand-drawn SVG and no libraries.
