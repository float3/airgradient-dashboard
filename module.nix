{
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.services.airgradient-dashboard;

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

  # Only the sides that are actually set reach the service; a null would be
  # read there as a bound of its own.
  limits =
    lib.mapAttrs
    (_: l: lib.filterAttrs (_: v: v != null) {inherit (l) min max;})
    cfg.limits;
in {
  options.services.airgradient-dashboard = {
    enable = lib.mkEnableOption "a local history and chart page for an AirGradient monitor";

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ./package.nix {};
      defaultText = lib.literalExpression "pkgs.callPackage ./package.nix {}";
      description = "The dashboard build to run.";
    };

    sensorUrl = lib.mkOption {
      type = lib.types.str;
      example = "http://airgradient_744dbdbfe7f4.local";
      description = ''
        Base URL of the monitor's local API (it serves /measures/current).

        Prefer a plain address over an mDNS name where you can: a `.local` name
        makes the dashboard depend on avahi being healthy as well as the sensor.
      '';
    };

    listenAddress = lib.mkOption {
      type = lib.types.str;
      default = "127.0.0.1";
      description = "Address the chart page binds to.";
    };

    port = lib.mkOption {
      type = lib.types.port;
      default = 8123;
      description = "Port for the chart page.";
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
      enable = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = ''
          Act on a measure leaving its range. Turning this off keeps the
          ranges, the red outline on the page and the `alarms` field of
          `/data`, and only stops this service making noise or running the
          hook.
        '';
      };

      device = lib.mkOption {
        type = lib.types.str;
        default = "";
        example = "/dev/input/by-path/platform-pcspkr-event-spkr";
        description = ''
          evdev node that accepts EV_SND tones, usually the PC speaker. Empty
          means no sound from this service.

          Not every machine that beeps at power-on can be made to beep from
          Linux: on many small-form-factor boards the POST beep comes from the
          embedded controller and the PIT speaker line drives nothing. Check
          with a long tone before relying on it.
        '';
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
          push notification. Anything that needs the room's sound hardware
          belongs in a user service instead: this one runs as a DynamicUser
          with no seat.
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

      consecutive = lib.mkOption {
        type = lib.types.ints.positive;
        default = 3;
        description = ''
          Readings in a row outside the range before the alarm sounds, and
          inside it again before it stops. Keeps one spike from crying wolf.
        '';
      };

      repeatSeconds = lib.mkOption {
        type = lib.types.ints.positive;
        default = 60;
        description = "How often the pattern repeats while the alarm stands.";
      };

      onStale = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = ''
          Also alarm when the sensor stops answering. A frozen chart otherwise
          looks exactly like calm air.
        '';
      };

      stalePolls = lib.mkOption {
        type = lib.types.ints.positive;
        default = 5;
        description = "Missed polls before the sensor counts as stale.";
      };
    };
  };

  config = lib.mkIf cfg.enable {
    # The beeper is a module, and nothing else on a headless box loads it.
    boot.kernelModules =
      lib.mkIf (cfg.alarm.enable && lib.hasInfix "pcspkr" cfg.alarm.device)
      ["pcspkr"];

    systemd.services.airgradient-dashboard = {
      description = "AirGradient history and charts";
      wantedBy = ["multi-user.target"];
      after = ["network-online.target"];
      wants = ["network-online.target"];
      environment =
        {
          AIRGRADIENT_URL = cfg.sensorUrl;
          LISTEN_ADDRESS = cfg.listenAddress;
          PORT = toString cfg.port;
          POLL_SECONDS = toString cfg.pollSeconds;
          RETENTION_HOURS = toString cfg.retentionHours;
          WINDOW_HOURS = toString cfg.windowHours;
          LIMITS = builtins.toJSON limits;
        }
        // lib.optionalAttrs cfg.alarm.enable {
          ALARM_DEVICE = cfg.alarm.device;
          ALARM_COMMAND = cfg.alarm.command;
          # The hook is a shell line written by whoever configures this, so
          # give it the system's own tools rather than systemd's bare default.
          PATH = "/run/current-system/sw/bin";
          ALARM_PATTERN = builtins.toJSON cfg.alarm.pattern;
          ALARM_CONSECUTIVE = toString cfg.alarm.consecutive;
          ALARM_REPEAT_SECONDS = toString cfg.alarm.repeatSeconds;
          ALARM_ON_STALE =
            if cfg.alarm.onStale
            then "1"
            else "0";
          STALE_POLLS = toString cfg.alarm.stalePolls;
        };
      serviceConfig =
        {
          ExecStart = lib.getExe cfg.package;
          DynamicUser = true;
          StateDirectory = "airgradient-dashboard";
          Restart = "always";
          RestartSec = 5;
        }
        # The beeper is an input device, owned by the input group.
        // lib.optionalAttrs (cfg.alarm.enable && cfg.alarm.device != "") {
          SupplementaryGroups = ["input"];
        };
    };
  };
}
