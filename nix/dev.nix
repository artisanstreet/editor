# `nix run .#dev`: the development loop.
#
# The app is the Linux dev runner (scripts/native_dev), compiled with Cargo
# in the checkout's `cargo-linux` dev shell into target/dev/linux-runner, so
# rerunning only recompiles what changed. Run from the checkout, it builds
# the Debug stage the same way (Production with Nix) and deploys both halves
# through the shipping installer: the Linux installation with its Forge as a
# systemd user service, and the Editor (inside WSL, the Windows build through
# interop), registered with that Forge and launched.
#
#   nix run .#dev                      Debug stage: build, install, launch
#   nix run .#dev -- --production      Production stage
#   nix run .#dev -- --nix             Debug stage, built in the Nix sandbox
#   nix run .#dev -- stage             install without launching the Editor
#   nix run .#dev -- where | prune     inspect or clean the installations
#   nix run .#dev -- --linux           the Linux Editor, also inside WSL
{
  pkgs,
  releaseTrust,
  graphicsLibraryPath,
}:
let
  # The runners' installer library compiles as a release build, so it
  # carries the public trust anchor like any release installer; the Linux
  # runner passes these on to the Windows runner's build and withholds them
  # from payload builds. The Linux runner also records where the Linux Editor
  # loads its graphics libraries from.
  runnerEnvironment = releaseTrust // {
    ARTISAN_DEV_GRAPHICS_LIBRARY_PATH = graphicsLibraryPath;
  };
  target = pkgs.stdenv.hostPlatform.rust.rustcTarget;
  runner = pkgs.writeShellApplication {
    name = "dev";
    runtimeInputs = [ pkgs.git ];
    text = ''
      if ! root="$(git rev-parse --show-toplevel 2>/dev/null)"; then
        echo "dev: error: run \`nix run .#dev\` inside the editor checkout" >&2
        exit 2
      fi
      # The default dev shell caps Cargo at two jobs for interactive use.
      unset CARGO_BUILD_JOBS CARGO_TARGET_DIR
      ${pkgs.lib.concatStrings (
        pkgs.lib.mapAttrsToList (name: value: ''
          export ${name}=${pkgs.lib.escapeShellArg value}
        '') runnerEnvironment
      )}
      target_dir="$root/target/dev/linux-runner"
      nix develop "$root#cargo-linux" --command cargo build --locked \
        --profile production-debug --target ${target} --target-dir "$target_dir" \
        -p artisan-native-dev --bin dev
      exec "$target_dir/${target}/production-debug/dev" "$@"
    '';
  };
in
{
  type = "app";
  program = "${runner}/bin/dev";
  meta.description = "Build the product and deploy the Forge service and the Editor as Artisan Street Dev";
}
