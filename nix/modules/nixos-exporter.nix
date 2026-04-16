# NixOS (Linux/systemd) module for the metalps Prometheus exporter.
# Exported from the flake as nixosModules.exporter.
# See darwin-exporter.nix for the macOS/launchd equivalent.
#
# Minimal usage (defaults to Unix domain socket):
#
#   inputs.metalps.nixosModules.exporter
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
#
# To reference the socket from a reverse proxy (e.g. nginx):
#
#   locations."/".proxyPass =
#     "http://unix:${config.services.prometheus.exporters.metalps.socket}";
#
# Note: when using socket mode the reverse proxy user must be a member of
# the service group (cfg.group) so it can connect to the socket.
{self}: {
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.services.prometheus.exporters.metalps;
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
      default = "/run/prometheus-metalps-exporter/prometheus-metalps-exporter.sock";
      description = ''
        Path for the Unix domain socket used by the exporter.  When set,
        systemd socket activation is used and the host/port options are
        ignored.  Set to null to use TCP instead.

        Other services (e.g. nginx) that proxy to this socket must be
        members of the service group to connect.
      '';
    };

    # host and port are separate options (rather than a single "listen"
    # string) so that other Nix expressions can reference them
    # individually — e.g. firewall rules need the port, reverse proxy
    # configs need host:port, and scrape configs need both.  The
    # module combines them into the --listen flag internally.
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
      default = "metalps-exporter";
      description = "System user account the exporter runs as.";
    };

    group = lib.mkOption {
      type = lib.types.str;
      default = "metalps-exporter";
      description = "System group the exporter runs as.";
    };

    openFirewall = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = ''
        Open the exporter's TCP port in the firewall.  Only applies when
        socket is null (TCP mode).
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    users.users.${cfg.user} = {
      isSystemUser = true;
      group = cfg.group;
      description = "metalps Prometheus exporter service user";
    };

    users.groups.${cfg.group} = {};

    # Create the socket directory before the socket unit tries to bind.
    systemd.tmpfiles.rules = lib.mkIf (cfg.socket != null) [
      "d ${dirOf cfg.socket} 0750 ${cfg.user} ${cfg.group} -"
    ];

    # Socket unit: systemd creates and holds the Unix domain socket, then
    # passes the open file descriptor to the service on first activation.
    systemd.sockets.prometheus-metalps-exporter = lib.mkIf (cfg.socket != null) {
      description = "metalps Prometheus exporter Unix domain socket";
      wantedBy = ["sockets.target"];
      socketConfig = {
        ListenStream = cfg.socket;
        SocketUser = cfg.user;
        SocketGroup = cfg.group;
        # 0660: accessible to the service user and group only.  Add the
        # reverse proxy user to cfg.group to grant it access.
        SocketMode = "0660";
        Accept = false;
      };
    };

    systemd.services.prometheus-metalps-exporter = {
      description = "metalps Prometheus GPU exporter";
      wantedBy = ["multi-user.target"];
      after =
        ["network.target"]
        ++ lib.optional (cfg.socket != null) "prometheus-metalps-exporter.socket";
      requires =
        lib.optional (cfg.socket != null) "prometheus-metalps-exporter.socket";

      environment = {
        LOG_LEVEL = cfg.logLevel;
        LOG_FORMAT = cfg.logFormat;
        METALPS_INTERVAL = toString cfg.intervalMs;
      };

      serviceConfig = {
        # Type = notify causes systemd to wait for the binary to call
        # sd_notify(READY=1) before marking the unit active.  The binary
        # does this via the sd-notify crate immediately after the listener
        # is bound.  NotifyAccess = main restricts who may send
        # notifications to the main process only.
        Type = "notify";
        NotifyAccess = "main";

        # Restart if no WATCHDOG=1 heartbeat arrives within 30 s.  The
        # binary reads WATCHDOG_USEC and pings at half this interval (15 s).
        WatchdogSec = lib.mkDefault "30s";

        ExecStart = let
          listenArg =
            if cfg.socket != null
            then "--listen sd-listen"
            else "--listen ${cfg.host}:${toString cfg.port}";
          extraArgs = lib.concatStringsSep " " cfg.extraFlags;
        in
          "${cfg.package}/bin/metalps-exporter"
          + " ${listenArg}"
          + " --interval-ms ${toString cfg.intervalMs}"
          + lib.optionalString (extraArgs != "") " ${extraArgs}";

        User = cfg.user;
        Group = cfg.group;
        Restart = "on-failure";
        RestartSec = "5s";

        # Harden the service environment.
        NoNewPrivileges = true;
        PrivateTmp = true;
        ProtectSystem = "strict";
        ProtectHome = true;
      };
    };

    networking.firewall.allowedTCPPorts =
      lib.mkIf (cfg.openFirewall && cfg.socket == null) [cfg.port];
  };
}
