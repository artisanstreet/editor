# `nix run .#dev`: build the apps and install them like a release.
#
# Builds `ae`, `forge` and `artisan-editor` with Cargo in the checkout's dev
# shells (`cargo-linux`, and `cargo-windows` for the Windows Editor),
# incremental and unsandboxed, into target/dev/<platform>, stages them in
# target/dev/stage/<os>/ with a VERSION (`<version>-dev+<commit>`), and runs
# `ae install --from target/dev/stage`. That is the one install on this
# machine, the same as a release: Forge in Artisan Street with its service,
# `ae` on PATH, the Editor with its Start menu entry. Inside WSL the Editor
# is the Windows build, installed on Windows by the Windows `ae`.
#
#   nix run .#dev                     build and install everything, restart Forge, reopen the Editor
#   nix run .#dev -- forge            build and install `ae` and Forge only
#   nix run .#dev -- editor           build and install `ae` and the Editor only
#   nix run .#dev -- build            build; nothing is installed
#   nix run .#devfast [-- <command>]   same pipeline, with Blaze for Linux builds
#                                      (Windows cross-builds still use Cargo;
#                                      Linux output stays in target/devfast)
#
#   --linux        inside WSL, install the Linux Editor (WSLg) instead of the Windows one
#   --dhat         build the Editor and Forge with the dhat heap profiler; the
#                  Editor writes logs/dhat-heap.json in its data folder when it
#                  quits, Forge dhat-heap.json in its own when asked to stop
#   ARTISAN_DEV_PROFILE   debug (default: dependencies optimised, workspace at
#                         opt-level 1) or release (what ships; slow to build)
#   CARGO_BUILD_JOBS      respected; the dev shell defaults to two jobs
#
#   nix run .#release [-- <command>] is the same loop with the release profile
#   (Cargo's `release`, what ships), built into target/dev/<platform>/release.
{
  pkgs,
  graphicsLibraryPath,
  profile ? "debug",
  blazePackage ? null,
}:
let
  runner = pkgs.writeShellApplication {
    name =
      if blazePackage != null then
        "devfast"
      else if profile == "debug" then
        "dev"
      else
        "dev-${profile}";
    runtimeInputs = with pkgs; [
      git
      coreutils
      gnused
      gnugrep
      procps
      util-linux
    ];
    text = ''
      ARTISAN_DEV_GRAPHICS_LIBRARY_PATH=${pkgs.lib.escapeShellArg graphicsLibraryPath}
      : "''${ARTISAN_DEV_PROFILE:=${profile}}"
      ARTISAN_DEV_BLAZE=${if blazePackage == null then "" else "${blazePackage}/bin/blaze"}
    ''
    + builtins.readFile ./dev.sh;
  };
in
{
  type = "app";
  program = "${runner}/bin/${runner.name}";
  meta.description = "Build (${profile} profile) and install Forge, the Editor and ae${
    if blazePackage == null then "" else " with Blaze"
  }";
}
