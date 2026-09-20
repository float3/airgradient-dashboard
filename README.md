# wall-dashboards

Full-screen panels for a display across the room: indoor air quality, weather,
and the next departures from your stops. One small Rust service serves them
all, polls upstream on its own schedule, and keeps working when upstream does
not.

It exists because the alternative is pointing a kiosk browser at somebody's
website, and websites log you out, move things around, show cookie dialogs and
ads, and are laid out for a desk rather than a wall.

## Panels

Every panel is optional. A host configures the ones it wants, and the rest are
neither polled nor served.

| Route | Source | Shows |
| --- | --- | --- |
| `/air` | An [AirGradient](https://www.airgradient.com/) monitor on your LAN | PM1/PM2.5/PM10, CO₂, TVOC, NOx, temperature and humidity over three hours, with an alarm |
| `/weather` | [Open-Meteo](https://open-meteo.com/) (no key) | Now, a daylight arc, a 24 h rain timeline, three days in detail and ten in outline |
| `/transit` | MVG's departures endpoint | Realtime departures per stop, with delays and cancellations |

`/` lists whichever are configured.

The polling happens in the service, not the page. A wall display usually
rotates through several pages, so any one panel is on screen a fraction of the
time; polling here means the air panel has a full history behind it the moment
it appears, and upstream is asked once however many browsers are looking.

## The air alarm

Each measure can be given a safe range. Three readings outside it raise the
alarm and three inside it clear the alarm, so one spike stays quiet. The chart
gets a red outline, the header says which measure and which way, and the
service can make a noise or run a hook.

A sensor that stopped answering is its own alarm. A frozen chart otherwise
looks exactly like calm air, which is the failure mode that matters.

Silence the sound without hiding the charts:

```sh
curl -X POST 'http://127.0.0.1:8123/air/mute?minutes=30'
curl -X POST http://127.0.0.1:8123/air/unmute
```

### Getting the alarm to somebody

Two hooks, neither assuming a sound card:

- `air.alarm.device` writes `EV_SND` tones to an evdev node, normally the PC
  speaker.
- `air.alarm.command` runs a shell line whenever the set of alarms *changes*,
  with `ALARM_STATE`, `ALARM_MEASURES` and `ALARM_TEXT` in the environment. It
  fires on transitions only, never on the repeat, which is what a push
  notification wants.

Anything needing the machine's sound hardware belongs in a user service polling
`/air/data`: this one runs as a `DynamicUser` with no seat and no access to
anyone's audio session.

**Do not assume the PC speaker works.** A machine that beeps at power-on often
cannot be made to beep from Linux: on many small-form-factor boards the POST
beep comes from the embedded controller and the PIT speaker line drives
nothing. The driver will accept your tones and report success while making no
sound at all. A BIOS pin config claiming an internal speaker proves nothing
either — the part is often simply not fitted. Test with a long continuous tone
before you rely on any of it.

## Configure it

```nix
{
  inputs.wall-dashboards.url = "github:float3/wall-dashboards";

  # in your host's modules:
  imports = [inputs.wall-dashboards.nixosModules.default];

  services.wall-dashboards = {
    enable = true;
    air = {
      enable = true;
      sensorUrl = "http://10.0.0.5";
      limits.rco2.max = 1000;
    };
    weather = {
      enable = true;
      latitude = 52.52;
      longitude = 13.41;
    };
    transit = {
      enable = true;
      stops = [
        {
          globalId = "de:09162:2"; # Marienplatz
          name = "Marienplatz";
          walkMinutes = 3;
        }
      ];
    };
  };
}
```

Find a stop's id with:

```sh
curl 'https://www.mvg.de/api/bgw-pt/v3/locations?query=<name>'
```

Every option is documented in [`module.nix`](module.nix). Without flakes it is
an ordinary NixOS module, and `package.nix` an ordinary `callPackage` file.

### Where you are stays yours

Coordinates, stop ids and the address of a sensor on your LAN say where you
live. None of them are in this repository, and none of them have defaults here:
they are required options, written by whoever deploys it, into a config file
this service reads at startup. Keep that config in a private repository.

## Notes on the air numbers

- **An AirGradient ONE has no oxygen sensor.** CO₂ is the only stand-in for a
  room going stuffy, which is why it carries the default range it does.
- **PM readings of exactly 0 are usually real.** The PMS5003 quantises mass to
  1 µg/m³ and averages a handful of samples, so in clean indoor air PM1 and
  PM10 sit at 0 and only the particle counts still move.
- **PM2.5 jumps between 0 and about 1.6.** The panel plots `pm02Compensated`,
  the humidity-corrected value, as AirGradient's own dashboard does. The
  firmware special-cases a raw 0 to 0 but maps a raw 1 to roughly 1.6, so the
  compensated trace has a gap it can never land in. It looks like noise and is
  not.

## Run it by hand

```sh
cargo run --release -- config.json
```

Then open <http://127.0.0.1:8123>. The config file is the only input; the
NixOS module is a typed front end that writes one.

The pages are baked into the binary, so the service is a single artifact with
nothing beside it to install. Each page is plain HTML with hand-drawn SVG and
no libraries.

## Adding a panel

A route in `src/main.rs`, a file in `pages/`, and a poller — `src/feed.rs` has
a cached one for anything that is just somebody else's JSON, which is most
things. `src/air.rs` is the shape to copy when a panel needs to keep history
or judge what it sees.
