# `nix run .#dev`: the development loop.
#
# Builds `ae` and `artisan-editor` with Cargo in the checkout's dev shells
# (`cargo-linux`, and `cargo-windows` for the Windows Editor), incremental
# and unsandboxed, into target/dev/<platform>, then runs them as a separate
# dev install that never touches a real one:
#
#   ARTISAN_HOME        $XDG_DATA_HOME/artisan-dev (override: ARTISAN_DEV_HOME)
#     bin/ae            the dev `ae` (also bin/artisan-editor on Linux)
#     forge/            the dev Forge's data (ARTISAN_FORGE_DATA); forge.log, forge.pid
#     editor/           the Linux Editor's data (ARTISAN_EDITOR_DATA)
#     wsl-env.sh        sourced by the bash that `wsl.exe -- ae …` starts for
#                       the dev Windows Editor (see below)
#
# The dev Forge listens on 127.0.0.1:47901 (ARTISAN_DEV_PORT), so it never
# collides with an installed Forge on 47900. It runs detached (`setsid`, pid
# file and log in forge/, like `ae forge start` without systemd); it is never
# a systemd unit, because `ae forge restart` would act on the real
# artisan-forge.service when one is installed.
#
# Inside WSL the Editor is the Windows build: copied to
# %LOCALAPPDATA%\Artisan Dev\ with its data in editor\ there and started by
# Explorer (outside WSL's interop job, so it outlives this terminal) through
# launch.cmd, which you can also double-click. launch.cmd passes BASH_ENV
# through WSLENV, so when the Editor's Home screen pairs with this
# distribution (`wsl.exe -d <distro> -- ae forge pair --json`), that bash
# sources wsl-env.sh and runs the dev `ae` against the dev Forge.
#
#   nix run .#dev                     build both, restart the dev Forge, relaunch the Editor
#   nix run .#dev -- forge            build `ae`, restart the dev Forge only
#   nix run .#dev -- editor           build and relaunch the Editor only
#   nix run .#dev -- stop             stop the dev Forge and the dev Editor
#   nix run .#dev -- import [--from DIR]
#                                     one-time: copy the old Forge's threads, skills and
#                                     engines into the dev Forge (`ae forge import`; the
#                                     old database is only read). Default DIR:
#                                     $XDG_DATA_HOME/Artisan Street Dev/data
#   nix run .#dev -- ae <args>        the dev `ae` with the dev environment,
#                                     e.g. `ae forge logs -f`, `ae forge devices`
#
#   --linux        inside WSL, run the Linux Editor (WSLg) instead of the Windows one
#   ARTISAN_DEV_PROFILE   debug (default: dependencies optimised, workspace at
#                         opt-level 1) or release (what ships; slow to build)
#   CARGO_BUILD_JOBS      unset by default, so Cargo uses every core
{
  pkgs,
  graphicsLibraryPath,
}:
let
  runner = pkgs.writeShellApplication {
    name = "dev";
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
    ''
    + builtins.readFile ./dev.sh;
  };
in
{
  type = "app";
  program = "${runner}/bin/dev";
  meta.description = "Build ae and the Editor and run them as the dev install (dev Forge, dev Editor)";
}
