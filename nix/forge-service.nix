# A Forge as a NixOS system service: `ae forge serve` with its data in
# /var/lib/artisan-forge. Pair an Editor with it on the host:
#
#   sudo -u artisan-forge ae --data-dir /var/lib/artisan-forge forge pair --listen <reachable address>
#
# Forge makes its own TLS certificate in the data directory on first start;
# no secret enters the Nix store.
self:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.services.artisan-forge;
  port = lib.toInt (lib.last (lib.splitString ":" cfg.listenAddress));
in
{
  options.services.artisan-forge = {
    enable = lib.mkEnableOption "an Artisan Forge";
    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.ae;
      defaultText = lib.literalExpression "artisan.packages.\${system}.ae";
      description = "The package providing `ae`.";
    };
    listenAddress = lib.mkOption {
      type = lib.types.str;
      default = "127.0.0.1:47900";
      example = "0.0.0.0:47900";
      description = "Address Forge listens on (HTTPS). Use a reachable interface for Editors on other machines.";
    };
    dataDir = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/artisan-forge";
      description = "Forge's data: database, certificate, engine installs and homes.";
    };
    openFirewall = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Open the listen port in the firewall.";
    };
    extraPackages = lib.mkOption {
      type = lib.types.listOf lib.types.package;
      default = [ ];
      description = "Tools the engines may run (git, bash and coreutils are always there).";
    };
  };
  config = lib.mkIf cfg.enable {
    users.groups.artisan-forge = { };
    users.users.artisan-forge = {
      isSystemUser = true;
      group = "artisan-forge";
      home = cfg.dataDir;
    };
    environment.systemPackages = [ cfg.package ];
    networking.firewall.allowedTCPPorts = lib.mkIf cfg.openFirewall [ port ];
    systemd.services.artisan-forge = {
      description = "Artisan Forge";
      wantedBy = [ "multi-user.target" ];
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
      path = [
        pkgs.git
        pkgs.bash
        pkgs.coreutils
        pkgs.getent
      ]
      ++ cfg.extraPackages;
      environment = {
        HOME = cfg.dataDir;
        ARTISAN_FORGE_DATA = cfg.dataDir;
      };
      serviceConfig = {
        ExecStart = lib.escapeShellArgs [
          (lib.getExe' cfg.package "ae")
          "forge"
          "serve"
          "--listen"
          cfg.listenAddress
        ];
        User = "artisan-forge";
        Group = "artisan-forge";
        StateDirectory = lib.mkIf (cfg.dataDir == "/var/lib/artisan-forge") "artisan-forge";
        StateDirectoryMode = "0700";
        UMask = "0077";
        Restart = "on-failure";
        RestartSec = 5;
        TimeoutStopSec = 30;
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ReadWritePaths = [ cfg.dataDir ];
        ProtectHome = true;
        PrivateTmp = true;
      };
    };
  };
}
