# Darwin (macOS/launchd) module for the metalps Prometheus exporter.
# Exported from the flake as darwinModules.exporter.
# See nixos-exporter.nix for the Linux/systemd equivalent.
#
# Minimal usage (defaults to Unix domain socket):
#
#   inputs.metalps.darwinModules.exporter
#
#   services.prometheus.exporters.metalps = {
#     enable = true;
#   };
#
# To use TCP instead:
#
#   services.prometheus.exporters.metalps = {
#     enable = true;
#     socket = null;
#     port   = 9101;
#   };
{self}: {
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.services.prometheus.exporters.metalps;

  listenArg =
    if cfg.socket != null
    then "--listen unix:${cfg.socket}"
    else "--listen ${cfg.host}:${toString cfg.port}";

  execLine =
    "${cfg.package}/bin/metalps-exporter"
    + " ${listenArg}"
    + " --interval-ms ${toString cfg.intervalMs}"
    + lib.optionalString (cfg.extraFlags != [])
      " ${lib.concatStringsSep " " cfg.extraFlags}";
in {
  options.services.prometheus.exporters.metalps = {
    enable = lib.mkEnableOption "metalps Prometheus GPU exporter";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.exporter;
      defaultText = lib.literalExpression "self.packages.\${system}.exporter";
      description = "Package providing the metalps-exporter binary.";
    };

    socket = lib.mkOption {
      type = lib.types.nullOr lib.types.path;
      default = "/var/run/prometheus-metalps-exporter/prometheus-metalps-exporter.sock";
      description = ''
        Path for the Unix domain socket used by the exporter.  When set,
        the server binds its own socket (no launchd socket activation) and
        the host/port options are ignored.  Set to null to use TCP instead.
      '';
    };

    # host and port are separate options (rather than a single "listen"
    # string) so that other Nix expressions can reference them
    # individually — e.g. scrape configs need host:port.  The module
    # combines them into the --listen flag internally.
    host = lib.mkOption {
      type = lib.types.str;
      default = "127.0.0.1";
      description = "IP address to bind to.  Ignored when socket is set.";
    };

    port = lib.mkOption {
      type = lib.types.port;
      default = 9101;
      description = "TCP port to listen on.  Ignored when socket is set.";
    };

    intervalMs = lib.mkOption {
      type = lib.types.int;
      default = 1000;
      description = "GPU sample interval in milliseconds.";
    };

    extraFlags = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [];
      description = "Extra command-line flags passed to metalps-exporter.";
    };

    logLevel = lib.mkOption {
      type = lib.types.enum ["trace" "debug" "info" "warn" "error"];
      default = "info";
      description = "Tracing log verbosity level.";
    };

    logFormat = lib.mkOption {
      type = lib.types.enum ["text" "json"];
      default = "json";
      description = ''
        Log output format.  Use "text" for human-readable local logs and
        "json" for structured logs consumed by a log aggregator.
      '';
    };

    user = lib.mkOption {
      type = lib.types.str;
      default = "_metalps-exporter";
      description = ''
        System user account the exporter runs as.  The leading underscore
        follows the macOS convention for daemon accounts.
      '';
    };

    group = lib.mkOption {
      type = lib.types.str;
      default = "_metalps-exporter";
      description = ''
        System group the exporter runs as.  The leading underscore follows
        the macOS convention for daemon groups.
      '';
    };

    uid = lib.mkOption {
      type = lib.types.int;
      default = 402;
      description = ''
        UID for the service user.  nix-darwin requires a static UID for
        user creation.  The default (402) sits above macOS Sequoia's
        claimed 300-304 range and below the 501 normal-user boundary.
      '';
    };

    gid = lib.mkOption {
      type = lib.types.int;
      default = 402;
      description = ''
        GID for the service group.  nix-darwin requires a static GID for
        group creation.  The default (402) mirrors the UID choice.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    users.users.${cfg.user} = {
      uid = cfg.uid;
      gid = cfg.gid;
      home = "/var/empty";
      shell = "/usr/bin/false";
      description = "metalps Prometheus exporter service user";
      isHidden = true;
    };

    users.groups.${cfg.group} = {
      gid = cfg.gid;
      members = [cfg.user];
    };

    users.knownUsers = [cfg.user];
    users.knownGroups = [cfg.group];

    # Create log and socket directories.  macOS has no tmpfiles equivalent,
    # so we use nix-darwin activation scripts.
    system.activationScripts.postActivation.text = let
      logDir = "/var/log/metalps-exporter";
      sockDir =
        if cfg.socket != null
        then dirOf cfg.socket
        else null;
    in
      ''
        mkdir -p ${logDir}
        chown ${cfg.user}:${cfg.group} ${logDir}
        chmod 0750 ${logDir}
      ''
      + lib.optionalString (sockDir != null) ''
        mkdir -p ${sockDir}
        chown ${cfg.user}:${cfg.group} ${sockDir}
        chmod 0750 ${sockDir}
      '';

    launchd.servers.prometheus-metalps-exporter = {
      serviceConfig = {
        ProgramArguments = [
          "/bin/sh"
          "-c"
          "/bin/wait4path ${cfg.package} && exec ${execLine}"
        ];
        UserName = cfg.user;
        GroupName = cfg.group;
        RunAtLoad = true;
        KeepAlive = {
          Crashed = true;
          SuccessfulExit = false;
        };
        ThrottleInterval = 30;
        ProcessType = "Background";
        EnvironmentVariables = {
          LOG_LEVEL = cfg.logLevel;
          LOG_FORMAT = cfg.logFormat;
          METALPS_INTERVAL = toString cfg.intervalMs;
        };
        StandardOutPath = "/var/log/metalps-exporter/stdout.log";
        StandardErrorPath = "/var/log/metalps-exporter/stderr.log";
      };
    };
  };
}
