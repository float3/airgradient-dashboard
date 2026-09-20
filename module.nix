{
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.services.wall-dashboards;

  limitModule = lib.types.submodule {
    options = {
      min = lib.mkOption {
        type = lib.types.nullOr lib.types.number;
        default = null;
        description = "Alarm when the measure falls below this.";
      };
      max = lib.mkOption {
        type = lib.types.nullOr lib.types.number;
        default = null;
        description = "Alarm when the measure rises above this.";
      };
    };
  };

  stopModule = lib.types.submodule {
    options = {
      globalId = lib.mkOption {
        type = lib.types.str;
        example = "de:09162:2"; # Marienplatz
        description = ''
          MVG's identifier for the stop. Find it with
          `curl 'https://www.mvg.de/api/bgw-pt/v3/locations?query=<name>'`.
        '';
      };
      name = lib.mkOption {
        type = lib.types.str;
        description = "What to call the stop on screen.";
      };
      limit = lib.mkOption {
        type = lib.types.ints.positive;
        default = 8;
        description = "How many departures to ask for.";
      };
      walkMinutes = lib.mkOption {
        type = lib.types.ints.unsigned;
        default = 0;
        description = ''
          Minutes on foot to the stop. Departures sooner than this are shown
          greyed out rather than hidden, so the board does not look emptier
          than the service really is.
        '';
      };
    };
  };

  # Only the sides that are actually set reach the service; a null would be
  # read there as a bound of its own.
  limits =
    lib.mapAttrs
    (_: l: lib.filterAttrs (_: v: v != null) {inherit (l) min max;})
    cfg.air.limits;

  settings =
    {
      inherit (cfg) listen port;
      stateDir = "/var/lib/${cfg.stateDirectory}";
    }
    // lib.optionalAttrs cfg.air.enable {
      air =
        {
          inherit (cfg.air) sensorUrl pollSeconds retentionHours windowHours;
          inherit limits;
        }
        // {alarm = cfg.air.alarm;};
    }
    // lib.optionalAttrs cfg.weather.enable {
      weather = {inherit (cfg.weather) latitude longitude timezone pollSeconds forecastDays;};
    }
    // lib.optionalAttrs cfg.transit.enable {
      transit = {inherit (cfg.transit) stops pollSeconds;};
    };

  configFile = pkgs.writeText "wall-dashboards.json" (builtins.toJSON settings);
in {
  options.services.wall-dashboards = {
    enable = lib.mkEnableOption "panels for a wall display";

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ./package.nix {};
      defaultText = lib.literalExpression "pkgs.callPackage ./package.nix {}";
      description = "The build to run.";
    };

    listen = lib.mkOption {
      type = lib.types.str;
      default = "127.0.0.1";
      description = "Address the panels bind to.";
    };

    port = lib.mkOption {
      type = lib.types.port;
      default = 8123;
      description = "Port for the panels.";
    };

    stateDirectory = lib.mkOption {
      type = lib.types.str;
      default = "wall-dashboards";
      description = "Name under /var/lib for history that outlives a restart.";
    };

    air = {
      enable = lib.mkEnableOption "the air-quality panel at /air";

      sensorUrl = lib.mkOption {
        type = lib.types.str;
        example = "http://10.0.0.5";
        description = ''
          Base URL of an AirGradient monitor's local API; it serves
          /measures/current.

          Prefer a plain address over an mDNS name where you can: a `.local`
          name makes the panel depend on avahi being healthy as well as on the
          sensor, and avahi is the more fragile of the two.
        '';
      };

      pollSeconds = lib.mkOption {
        type = lib.types.ints.positive;
        default = 30;
      };

      retentionHours = lib.mkOption {
        type = lib.types.ints.positive;
        default = 24;
        description = "How much history is kept on disk.";
      };

      windowHours = lib.mkOption {
        type = lib.types.numbers.positive;
        default = 3;
        description = "How much history the page draws.";
      };

      limits = lib.mkOption {
        type = lib.types.attrsOf limitModule;
        description = ''
          Safe range per measure, keyed by `pm01`, `pm02`, `pm10`, `rco2`,
          `tvoc`, `nox`, `atmp` or `rhum`. A measure that is left out is never
          alarmed on. Note that an AirGradient ONE has no oxygen sensor; CO₂ is
          the only stand-in for a room going stuffy.
        '';
        default = {
          rco2.max = 1000; # ppm; above this a room reads as under-ventilated
          atmp = {
            min = 18;
            max = 27;
          };
          rhum = {
            min = 30;
            max = 60;
          };
          pm02.max = 15; # µg/m³, the WHO 24 h guideline
          pm10.max = 45; # µg/m³, likewise
          tvoc.max = 250; # Sensirion index, 100 is typical indoor air
          nox.max = 50; # Sensirion index, 1 is typical indoor air
        };
      };

      alarm = {
        consecutive = lib.mkOption {
          type = lib.types.ints.positive;
          default = 3;
          description = ''
            Readings in a row outside the range before the alarm is raised,
            and inside it again before it clears. Keeps one spike from crying
            wolf.
          '';
        };

        repeatSeconds = lib.mkOption {
          type = lib.types.ints.positive;
          default = 60;
          description = "How often the sound repeats while the alarm stands.";
        };

        device = lib.mkOption {
          type = lib.types.str;
          default = "";
          example = "/dev/input/by-path/platform-pcspkr-event-spkr";
          description = ''
            evdev node that accepts EV_SND tones, usually the PC speaker.
            Empty means no sound from this service.

            Not every machine that beeps at power-on can be made to beep from
            Linux: on many small-form-factor boards the POST beep comes from
            the embedded controller and the PIT speaker line drives nothing.
            The driver will accept tones and report success while making no
            sound, so test with a long continuous tone before relying on it.
          '';
        };

        pattern = lib.mkOption {
          type = lib.types.listOf (lib.types.listOf lib.types.int);
          default = [
            [880 200]
            [0 120]
            [880 200]
            [0 120]
            [880 200]
          ];
          description = "Tones as [hertz milliseconds] pairs; 0 hertz is a rest.";
        };

        command = lib.mkOption {
          type = lib.types.lines;
          default = "";
          example = ''
            curl -fsS -H "Title: Air quality" -d "$ALARM_TEXT" https://ntfy.sh/my-topic
          '';
          description = ''
            Shell run whenever the set of alarms changes, with `ALARM_STATE`
            (`raised`, `changed` or `cleared`), `ALARM_MEASURES`
            (`rco2=high,atmp=low`) and `ALARM_TEXT` in the environment.

            This fires on transitions only, never on the repeat, so it suits a
            push notification. Anything that needs the machine's sound
            hardware belongs in a user service polling `/air/data` instead:
            this one runs as a DynamicUser with no seat.
          '';
        };

        onStale = lib.mkOption {
          type = lib.types.bool;
          default = true;
          description = ''
            Also alarm when the sensor stops answering. A frozen chart
            otherwise looks exactly like calm air.
          '';
        };

        stalePolls = lib.mkOption {
          type = lib.types.ints.positive;
          default = 5;
          description = "Missed polls before the sensor counts as stale.";
        };
      };
    };

    weather = {
      enable = lib.mkEnableOption "the weather panel at /weather";

      latitude = lib.mkOption {
        type = lib.types.float;
        example = 52.52;
        description = "Latitude to forecast for. Kept here, never in the source.";
      };

      longitude = lib.mkOption {
        type = lib.types.float;
        example = 13.41;
        description = "Longitude to forecast for. Kept here, never in the source.";
      };

      timezone = lib.mkOption {
        type = lib.types.str;
        default = config.time.timeZone or "auto";
        defaultText = lib.literalExpression "config.time.timeZone";
        description = "Timezone the forecast's days and sun times are given in.";
      };

      pollSeconds = lib.mkOption {
        type = lib.types.ints.positive;
        default = 900;
        description = ''
          Open-Meteo updates its models hourly, so a quarter hour is plenty
          and stays well inside what the free service asks of callers.
        '';
      };

      forecastDays = lib.mkOption {
        type = lib.types.ints.between 1 16;
        default = 10;
        description = "How far the low-detail outlook strip reaches.";
      };
    };

    transit = {
      enable = lib.mkEnableOption "the departures panel at /transit";

      stops = lib.mkOption {
        type = lib.types.listOf stopModule;
        default = [];
        description = "Stops to show, side by side, in this order.";
      };

      pollSeconds = lib.mkOption {
        type = lib.types.ints.positive;
        default = 30;
      };
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = !cfg.transit.enable || cfg.transit.stops != [];
        message = "services.wall-dashboards.transit needs at least one stop.";
      }
    ];

    # The beeper is a module, and nothing else on a headless box loads it.
    boot.kernelModules =
      lib.mkIf (cfg.air.enable && lib.hasInfix "pcspkr" cfg.air.alarm.device)
      ["pcspkr"];

    systemd.services.wall-dashboards = {
      description = "Panels for a wall display";
      wantedBy = ["multi-user.target"];
      after = ["network-online.target"];
      wants = ["network-online.target"];
      # The alarm hook is a shell line written by whoever configures this, so
      # give it the system's own tools rather than systemd's bare default. Add
      # packages here from your own configuration if the hook needs more.
      path =
        lib.optional (cfg.air.enable && cfg.air.alarm.command != "")
        "/run/current-system/sw";
      serviceConfig =
        {
          ExecStart = "${lib.getExe cfg.package} ${configFile}";
          DynamicUser = true;
          StateDirectory = cfg.stateDirectory;
          Restart = "always";
          RestartSec = 5;
        }
        # The beeper is an input device, owned by the input group.
        // lib.optionalAttrs (cfg.air.enable && cfg.air.alarm.device != "") {
          SupplementaryGroups = ["input"];
        };
    };
  };
}
