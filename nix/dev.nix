# `nix run .#dev`: the development loop, orchestrated by Nix.
#
# Nix builds the payload for the chosen platform and stage from the checkout
# (a dirty tree builds its working copy of tracked files), then the runner
# installs it as a signed dev-channel release into the per-user
# `Artisan Street Dev` installation and launches it, retiring any previous
# dev Editor first. Inside WSL the default platform is Windows: the Windows
# runner and payload are cross-built here and run through WSL interop.
#
#   nix run .#dev                      Debug stage: install and launch
#   nix run .#dev -- --production      Production stage
#   nix run .#dev -- stage             install without launching
#   nix run .#dev -- where | prune     inspect or clean the dev installation
#   nix run .#dev -- --linux           Linux build, also inside WSL
{
  lib,
  pkgs,
  linuxRunner,
  graphicalEnvironment,
}:
pkgs.writeShellApplication {
  name = "artisan-dev";
  runtimeInputs = [
    pkgs.git
    pkgs.coreutils
  ];
  text = graphicalEnvironment + ''
    stage=debug
    platform=auto
    command=run
    runner_arguments=()
    while [ "$#" -gt 0 ]; do
      case "$1" in
        --production) stage=production ;;
        --debug) stage=debug ;;
        --linux) platform=linux ;;
        --windows) platform=windows ;;
        run | stage | where | prune) command="$1" ;;
        *) runner_arguments+=("$1") ;;
      esac
      shift
    done
    if [ "$platform" = auto ]; then
      if [ -e /proc/sys/fs/binfmt_misc/WSLInterop ] || [ -e /proc/sys/fs/binfmt_misc/WSLInterop-late ]; then
        platform=windows
      else
        platform=linux
      fi
    fi

    checkout="$(git rev-parse --show-toplevel 2>/dev/null)" || {
      echo "artisan-dev: run inside the editor checkout" >&2
      exit 2
    }
    # Flakes see tracked files only; a new source file that is not yet
    # tracked would silently be missing from the build.
    untracked="$(git -C "$checkout" ls-files --others --exclude-standard -- '*.rs' '*.toml' '*.nix' | head -5)"
    if [ -n "$untracked" ]; then
      echo "artisan-dev: these files are untracked, so Nix cannot see them:" >&2
      echo "$untracked" | sed 's/^/  /' >&2
      echo "artisan-dev: track them with 'git add -N <path>' and rerun" >&2
      exit 2
    fi

    if [ "$platform" = windows ]; then
      runner="$(nix build --no-link --print-out-paths "$checkout#windows-runner")/bin/dev.exe"
    else
      runner=${linuxRunner}/bin/dev
    fi
    if [ "$command" = run ] || [ "$command" = stage ]; then
      echo "artisan-dev: building $platform-$stage" >&2
      payload="$(nix build --no-link --print-out-paths "$checkout#$platform-$stage")"
      if [ "$platform" = windows ]; then
        payload="$(wslpath -w "$payload")"
      fi
      runner_arguments=(--payload "$payload" "''${runner_arguments[@]}")
    fi
    exec "$runner" "$command" "''${runner_arguments[@]}"
  '';
  meta.description = "Build a stage with Nix, install it as Artisan Street Dev, and launch it";
}
