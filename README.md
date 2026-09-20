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

### Why the PC speaker

On the host this was written for, every sound card is wired to an FM
transmitter, so an alarm played through a sink would have been broadcast
instead of heard. The motherboard beeper is the one output that is always in
the room. Set `services.airgradient-dashboard.alarm.device` if yours is
somewhere else, or `alarm.enable = false` to keep only the red outline.

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
AIRGRADIENT_URL=http://192.168.1.80 STATE_DIRECTORY=/tmp/ag python3 server.py
```

Then open <http://127.0.0.1:8123>. The server is Python standard library only —
no dependencies — and the page is plain HTML with hand-drawn SVG, no libraries.
