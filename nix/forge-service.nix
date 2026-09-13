# Opt-in only. Supply provisioned runtime credentials and the complete Forge
# scheduler/listener policy; no secret bytes are evaluated into the store.
self:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.services.artisan-forge;
  credential = name: "/run/credentials/artisan-forge.service/${name}";
  args = [
    "--listen"
    cfg.listenAddress
    "--database"
    "/var/lib/artisan-forge/forge.db"
    "--custody"
    "/var/lib/artisan-forge/custody"
    "--ready-file"
    "/run/artisan-forge/ready.json"
    "--certificate-der"
    (credential "certificate.der")
    "--private-key-der"
    (credential "private-key.der")
    "--bootstrap-capability"
    (credential "bootstrap")
  ]
  ++ lib.concatLists (
    lib.mapAttrsToList (name: value: [
      "--${name}"
      (toString value)
    ]) cfg.policy
  );
  requiredPolicy = [
    "admission-timeout-ms"
    "handshake-timeout-ms"
    "request-timeout-ms"
    "drain-timeout-ms"
    "admission-capacity"
    "requests-per-connection"
    "native-run-claim-lease-ms"
    "native-run-poll-interval-ms"
    "native-run-retry-backoff-ms"
    "native-run-shutdown-budget-ms"
    "native-run-queue-capacity"
    "native-run-max-command-retries"
    "native-run-prompt-delivery"
    "native-run-stream-after"
  ];
in
{
  options.services.artisan-forge = {
    enable = lib.mkEnableOption "the explicitly provisioned Artisan Forge service";
    listenAddress = lib.mkOption {
      type = lib.types.str;
      default = "127.0.0.1:0";
      description = "Explicit QUIC UDP listening address; configure a reachable interface for remote hosts.";
    };
    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.forge;
      description = "Forge executable package.";
    };
    certificateFile = lib.mkOption {
      type = lib.types.str;
      description = "Absolute runtime path to leaf certificate DER (not a Nix path).";
    };
    privateKeyFile = lib.mkOption {
      type = lib.types.str;
      description = "Absolute runtime path to PKCS#8 private key DER (not a Nix path).";
    };
    bootstrapCapabilityFile = lib.mkOption {
      type = lib.types.str;
      description = "Absolute runtime path to the provisioned capability (not a Nix path).";
    };
    policy = lib.mkOption {
      type = lib.types.attrsOf (lib.types.either lib.types.str lib.types.ints.unsigned);
      description = "All Forge listener and native-run options, without the -- prefix. Required explicitly; no deployment defaults.";
      example = {
        admission-timeout-ms = 5000;
        handshake-timeout-ms = 5000;
        request-timeout-ms = 30000;
        drain-timeout-ms = 5000;
        admission-capacity = 32;
        requests-per-connection = 32;
        native-run-claim-lease-ms = 30000;
        native-run-poll-interval-ms = 100;
        native-run-retry-backoff-ms = 500;
        native-run-shutdown-budget-ms = 5000;
        native-run-queue-capacity = 32;
        native-run-max-command-retries = 3;
        native-run-prompt-delivery = "stdin";
        native-run-stream-after = "0";
      };
    };
  };
  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion =
          lib.sort builtins.lessThan (builtins.attrNames cfg.policy)
          == lib.sort builtins.lessThan requiredPolicy;
        message = "services.artisan-forge.policy must supply exactly every Forge listener/native-run option.";
      }
      {
        assertion = builtins.all (path: lib.hasPrefix "/" path && !(lib.hasPrefix "/nix/store/" path)) [
          cfg.certificateFile
          cfg.privateKeyFile
          cfg.bootstrapCapabilityFile
        ];
        message = "Forge credentials must be absolute runtime paths outside the Nix store.";
      }
    ];
    users.groups.artisan-forge = { };
    users.users.artisan-forge = {
      isSystemUser = true;
      group = "artisan-forge";
    };
    systemd.services.artisan-forge = {
      description = "Artisan Forge";
      path = [
        pkgs.git
        pkgs.getent
      ];
      environment.HOME = "/var/lib/artisan-forge";
      wantedBy = [ "multi-user.target" ];
      after = [ "network.target" ];
      serviceConfig = {
        ExecStart = lib.escapeShellArgs ([ "${cfg.package}/bin/forge" ] ++ args);
        User = "artisan-forge";
        Group = "artisan-forge";
        StateDirectory = "artisan-forge";
        StateDirectoryMode = "0700";
        RuntimeDirectory = "artisan-forge";
        RuntimeDirectoryMode = "0700";
        LoadCredential = [
          "certificate.der:${cfg.certificateFile}"
          "private-key.der:${cfg.privateKeyFile}"
          "bootstrap:${cfg.bootstrapCapabilityFile}"
        ];
        UMask = "0077";
        Restart = "on-failure";
        RestartSec = 5;
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
      };
    };
  };
}
