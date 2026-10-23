# A Forge as a NixOS system service: `forge` running as the artisan-forge
# user, its data under that user's home
# (/var/lib/artisan-forge/.local/share/artisan-street/forge/data). Pair an
# Editor with it on the host:
#
#   sudo -u artisan-forge -H ae pair
#
# Forge makes its own TLS certificate in its data on first start; no secret
# enters the Nix store.
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
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.forge;
      defaultText = lib.literalExpression "artisan.packages.\${system}.forge";
      description = "The package providing `forge`.";
    };
    aePackage = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.ae;
      defaultText = lib.literalExpression "artisan.packages.\${system}.ae";
      description = "The package providing `ae` (pairing, devices, engines).";
    };
    listenAddress = lib.mkOption {
      type = lib.types.str;
      default = "127.0.0.1:47900";
      example = "0.0.0.0:47900";
      description = "Address Forge listens on (HTTPS). Use a reachable interface for Editors on other machines.";
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
      home = "/var/lib/artisan-forge";
    };
    environment.systemPackages = [ cfg.aePackage ];
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
      environment.HOME = "/var/lib/artisan-forge";
      # `ae pair` reads the listen address from here too.
      preStart = ''
        data="$HOME/.local/share/artisan-street/forge/data"
        mkdir -p "$data"
        printf '{"listen":"%s"}\n' ${lib.escapeShellArg cfg.listenAddress} > "$data/config.json"
      '';
      serviceConfig = {
        ExecStart = lib.getExe' cfg.package "forge";
        User = "artisan-forge";
        Group = "artisan-forge";
        StateDirectory = "artisan-forge";
        StateDirectoryMode = "0700";
        UMask = "0077";
        Restart = "on-failure";
        RestartSec = 5;
        TimeoutStopSec = 30;
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ReadWritePaths = [ "/var/lib/artisan-forge" ];
        ProtectHome = true;
        PrivateTmp = true;
      };
    };
  };
}
