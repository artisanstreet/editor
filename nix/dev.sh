# shellcheck shell=bash
# The dev loop behind `nix run .#dev`; usage is in nix/dev.nix.
# nix/dev.nix prepends ARTISAN_DEV_GRAPHICS_LIBRARY_PATH.
: "${ARTISAN_DEV_GRAPHICS_LIBRARY_PATH:=}"

say() { echo "dev: $*" >&2; }
die() {
    echo "dev: error: $*" >&2
    exit 1
}

root="$(git rev-parse --show-toplevel 2>/dev/null)" || die "run \`nix run .#dev\` inside the editor checkout"
[[ -f "$root/crates/ae/Cargo.toml" ]] || die "$root is not the Artisan checkout"

usage() {
    sed -n '1,/^{/{/^#/p}' "$root/nix/dev.nix"
    exit 0
}

command=run
linux=false
dhat=false
while (($#)); do
    case "$1" in
        build | run | forge | editor) command="$1" ;;
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

# The default dev shell caps Cargo at two jobs for interactive use.
unset CARGO_TARGET_DIR CARGO_BUILD_TARGET

# cargo_build <dev shell> <platform> <cargo args>: each platform has its own
# target directory, since the shells' environments differ.
cargo_build() {
    local shell="$1" platform="$2"
    shift 2
    say "building $* ($profile, $platform)"
    (cd "$root" && nix develop "$root#$shell" --command \
        cargo build --profile "$profile" --target-dir "$root/target/dev/$platform" "$@")
}
linux_out="$root/target/dev/linux/$profile_dir"
windows_out="$root/target/dev/windows/x86_64-pc-windows-gnu/$profile_dir"
stage="$root/target/dev/stage"

# What gets built and where the Editor goes.
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
editor_args=(-p editor --bin artisan-editor)
if [[ "$dhat" == true ]]; then editor_args+=(--features editor/dhat-heap); fi
if [[ "$want_editor" == true && "$windows" == false ]]; then linux_args+=("${editor_args[@]}"); fi
cargo_build cargo-linux linux "${linux_args[@]}"
if [[ "$windows" == true ]]; then
    windows_args=(-p ae --bin ae)
    if [[ "$want_editor" == true ]]; then windows_args+=("${editor_args[@]}"); fi
    cargo_build cargo-windows windows "${windows_args[@]}"
fi

if [[ "$command" == build ]]; then
    say "build complete; binaries are in target/dev (nothing installed)"
    exit 0
fi

# A local build for `ae install --from`: a folder per OS with the binaries
# and their version.
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -n1)"
commit="$(git -C "$root" rev-parse --short=10 HEAD)"
if ! git -C "$root" diff --quiet HEAD 2>/dev/null; then commit="$commit.dirty"; fi
rm -rf "$stage"
mkdir -p "$stage/linux"
echo "$version-dev+$commit" > "$stage/linux/VERSION"
for binary in ae forge artisan-editor; do
    if [[ -f "$linux_out/$binary" ]]; then cp "$linux_out/$binary" "$stage/linux/"; fi
done
if [[ "$windows" == true ]]; then
    mkdir -p "$stage/windows"
    echo "$version-dev+$commit" > "$stage/windows/VERSION"
    if [[ -f "$windows_out/ae.exe" ]]; then cp "$windows_out/ae.exe" "$stage/windows/"; fi
    # Installed as `Artisan Editor.exe`, the name Windows shows.
    if [[ -f "$windows_out/artisan-editor.exe" ]]; then
        cp "$windows_out/artisan-editor.exe" "$stage/windows/Artisan Editor.exe"
    fi
fi

forge_place=none
editor_place=none
if [[ "$want_forge" == true ]]; then forge_place=here; fi
if [[ "$want_editor" == true ]]; then
    if [[ "$windows" == true ]]; then editor_place=windows; else editor_place=here; fi
fi

# The Linux Editor finds its graphics libraries through the environment
# `ae` starts it with; nothing else gets them (they break system tools).
if [[ "$editor_place" == here ]]; then
    library_path="$ARTISAN_DEV_GRAPHICS_LIBRARY_PATH"
    for dir in /usr/lib/wsl/lib /run/opengl-driver/lib; do
        if [[ -d "$dir" ]]; then library_path="$library_path:$dir"; fi
    done
    export LD_LIBRARY_PATH="$library_path${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
else
    unset LD_LIBRARY_PATH
fi
"$stage/linux/ae" install --from "$stage" --forge "$forge_place" --editor "$editor_place" --yes
