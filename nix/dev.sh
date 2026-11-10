# shellcheck shell=bash
: "${ARTISAN_DEV_BLAZE:=}"

say() { echo "dev: $*" >&2; }
die() {
    echo "dev: error: $*" >&2
    exit 1
}

root="$(git rev-parse --show-toplevel 2>/dev/null)" || die "run \`nix run .#dev\` inside the editor checkout"
[[ -f "$root/crates/ae/Cargo.toml" ]] || die "$root is not the Artisan checkout"

usage() {
    cat <<'USAGE'
Usage: nix run .#dev [-- COMMAND [OPTIONS]]

run       Build and install Forge and the native Windows Aqua Editor.
forge     Build and install Forge and ae. New turns use the replacement;
          existing turns keep running on their current generation.
editor    Build and install the native Windows Aqua Editor and ae.
build     Build without installing.

--client-only  Build only Aqua and open a new client without installing.
               Each launch uses its own executable copy; existing clients
               and Forge keep running. With build, skip opening the client.

--dhat    Enable Forge heap profiling (Forge-only builds).
--linux   Forge-only builds on Linux; the Aqua Editor requires Windows.

Replacement readiness is checked before traffic is handed over. A failed
candidate leaves the current Forge running. The first installation of the
upgrade-capable service requires one bootstrap restart.
USAGE
    exit 0
}

command=run
linux=false
dhat=false
client_only=false
while (($#)); do
    case "$1" in
        build | run | forge | editor) command="$1" ;;
        --client-only) client_only=true ;;
        --linux) linux=true ;;
        --dhat) dhat=true ;;
        -h | --help | help) usage ;;
        *) die "unknown argument '$1' (see \`nix run .#dev -- --help\`)" ;;
    esac
    shift
done

windows=false
if [[ "$linux" == false ]] && [[ -e /proc/sys/fs/binfmt_misc/WSLInterop || -e /proc/sys/fs/binfmt_misc/WSLInterop-late ]]; then
    windows=true
fi

case "${ARTISAN_DEV_PROFILE:-debug}" in
    debug) profile=dev profile_dir=debug ;;
    release) profile=release profile_dir=release ;;
    *) die "ARTISAN_DEV_PROFILE is debug or release, not '$ARTISAN_DEV_PROFILE'" ;;
esac

unset CARGO_TARGET_DIR CARGO_BUILD_TARGET
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"

output_root="$root/target/dev"
if [[ -n "$ARTISAN_DEV_BLAZE" ]]; then
    output_root="$root/target/devfast"
fi

cargo_build() {
    local shell="$1" platform="$2"
    shift 2
    local builder=cargo target_dir="$output_root/$platform"
    if [[ -n "$ARTISAN_DEV_BLAZE" && "$platform" == linux ]]; then builder="$ARTISAN_DEV_BLAZE"; fi
    if [[ -n "$ARTISAN_DEV_BLAZE" && "$platform" == windows ]]; then
        say "using Cargo and the regular dev cache for the Windows cross-build"
        target_dir="$root/target/dev/windows"
    fi
    say "building $* ($profile, $platform, $(basename "$builder"))"
    if [[ "$builder" != cargo && "$profile" == dev ]]; then
        (cd "$root" && CARGO_TARGET_DIR="$target_dir" nix develop "$root#$shell" --command \
            "$builder" build "$@")
    else
        (cd "$root" && nix develop "$root#$shell" --command \
            "$builder" build --profile "$profile" --target-dir "$target_dir" "$@")
    fi
}
linux_out="$output_root/linux/$profile_dir"
windows_out="$output_root/windows/x86_64-pc-windows-gnu/$profile_dir"
if [[ -n "$ARTISAN_DEV_BLAZE" ]]; then windows_out="$root/target/dev/windows/x86_64-pc-windows-gnu/$profile_dir"; fi
stage="$output_root/stage"

want_forge=true
want_editor=true
case "$command" in
    forge) want_editor=false ;;
    editor) want_forge=false ;;
esac

linux_args=(-p ae --bin ae)
if [[ "$want_forge" == true ]]; then
    linux_args+=(-p forge --bin forge)
    if [[ "$dhat" == true ]]; then linux_args+=(--features forge/dhat-heap); fi
fi
editor_args=(-p editor-aqua --bin editor-aqua)
if [[ "$client_only" == false ]]; then editor_args+=(--features capture); fi
if [[ "$want_editor" == true && "$windows" == false ]]; then die "the native Aqua Editor requires Windows; use 'forge' for a Forge-only build"; fi
if [[ "$dhat" == true && "$want_editor" == true ]]; then die "--dhat is supported only for Forge builds"; fi

if [[ "$client_only" == true ]]; then
    [[ "$command" != forge ]] || die "--client-only cannot be combined with forge"
    if [[ "$command" != build ]]; then
        command -v wslpath >/dev/null 2>&1 || die "--client-only requires wslpath"
        command -v powershell.exe >/dev/null 2>&1 || die "--client-only requires Windows PowerShell on PATH"
    fi
    cargo_build cargo-windows windows "${editor_args[@]}"
    if [[ "$command" == build ]]; then
        say "client build complete; binary is in $windows_out/editor-aqua.exe"
        exit 0
    fi
    [[ -f "$windows_out/editor-aqua.exe" ]] || die "native Aqua Editor build output is missing"
    source_path="$(wslpath -w "$windows_out/editor-aqua.exe")"
    source_literal="${source_path//\'/\'\'}"
    unset LD_LIBRARY_PATH
    powershell.exe -NoProfile -NonInteractive -Command "
        \$ErrorActionPreference = 'Stop'
        \$directory = Join-Path ([IO.Path]::GetTempPath()) ('artisan-editor-dev-' + [Guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path \$directory | Out-Null
        \$executable = Join-Path \$directory 'Artisan Editor Dev.exe'
        Copy-Item -LiteralPath '$source_literal' -Destination \$executable
        \$client = Start-Process -FilePath \$executable -WorkingDirectory \$directory -PassThru
        Write-Output ('dev: opened client PID ' + \$client.Id + ' from ' + \$executable)
    "
    exit 0
fi
if [[ -n "$ARTISAN_DEV_BLAZE" ]]; then
    cargo_build cargo-linux linux -p ae --bin ae
    if [[ "$want_forge" == true ]]; then
        forge_args=(-p forge --bin forge)
        if [[ "$dhat" == true ]]; then forge_args+=(--features forge/dhat-heap); fi
        cargo_build cargo-linux linux "${forge_args[@]}"
    fi
else
    cargo_build cargo-linux linux "${linux_args[@]}"
fi
if [[ "$windows" == true && "$want_editor" == true ]]; then
    windows_args=(-p ae --bin ae)
    if [[ "$want_editor" == true ]]; then windows_args+=("${editor_args[@]}"); fi
    cargo_build cargo-windows windows "${windows_args[@]}"
fi

if [[ "$command" == build ]]; then
    say "build complete; binaries are in $output_root (nothing installed)"
    exit 0
fi

version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -n1)"
commit="$(git -C "$root" rev-parse --short=10 HEAD)"
if ! git -C "$root" diff --quiet HEAD 2>/dev/null; then commit="$commit.dirty"; fi
rm -rf "$stage"
mkdir -p "$stage/linux"
echo "$version-dev+$commit" > "$stage/linux/VERSION"
for binary in ae forge; do
    if [[ -f "$linux_out/$binary" ]]; then cp "$linux_out/$binary" "$stage/linux/"; fi
done
if [[ "$windows" == true && "$want_editor" == true ]]; then
    mkdir -p "$stage/windows"
    echo "$version-dev+$commit" > "$stage/windows/VERSION"
    if [[ -f "$windows_out/ae.exe" ]]; then cp "$windows_out/ae.exe" "$stage/windows/"; fi
    if [[ "$want_editor" == true ]]; then
        [[ -f "$windows_out/editor-aqua.exe" ]] || die "native Aqua Editor build output is missing"
        cp "$windows_out/editor-aqua.exe" "$stage/windows/Artisan Editor.exe"
    fi
fi

forge_place=none
editor_place=none
if [[ "$want_forge" == true ]]; then forge_place=here; fi
if [[ "$want_editor" == true ]]; then
    editor_place=windows
fi

unset LD_LIBRARY_PATH
install_args=("$stage/linux/ae" install --from "$stage" --forge "$forge_place" --editor "$editor_place" --yes)
if [[ "$want_forge" == true ]] && command -v systemd-run >/dev/null 2>&1 && systemctl --user show-environment >/dev/null 2>&1; then
    say "installing through an independent update job; running agents stay with their current Forge generation"
    systemd-run --user --collect --wait --unit="artisan-dev-install-$(date +%s)-$$" \
        --working-directory="$root" --setenv="PATH=$PATH" "${install_args[@]}"
else
    "${install_args[@]}"
fi
