# shellcheck shell=bash
# The dev loop behind `nix run .#dev`; usage and layout are in nix/dev.nix.
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
from=""
while (($#)); do
    case "$1" in
        run | forge | editor | stop | import) command="$1" ;;
        ae)
            command=ae
            shift
            break
            ;;
        --linux) linux=true ;;
        --from)
            from="${2:?--from needs a directory}"
            shift
            ;;
        --from=*) from="${1#--from=}" ;;
        -h | --help | help) usage ;;
        *) die "unknown argument '$1' (see \`nix run .#dev -- --help\`)" ;;
    esac
    shift
done

export ARTISAN_HOME="${ARTISAN_DEV_HOME:-${XDG_DATA_HOME:-$HOME/.local/share}/artisan-dev}"
export ARTISAN_FORGE_DATA="$ARTISAN_HOME/forge"
export ARTISAN_FORGE_LISTEN="127.0.0.1:${ARTISAN_DEV_PORT:-47901}"
bin="$ARTISAN_HOME/bin"
ae="$bin/ae"
editor="$bin/artisan-editor"
pid_file="$ARTISAN_FORGE_DATA/forge.pid"
forge_log="$ARTISAN_FORGE_DATA/forge.log"

if [[ "$command" == ae ]]; then
    [[ -x "$ae" ]] || die "there is no dev ae yet; run \`nix run .#dev\` first"
    exec "$ae" "$@"
fi

windows=false
if [[ "$linux" == false ]] && [[ -e /proc/sys/fs/binfmt_misc/WSLInterop || -e /proc/sys/fs/binfmt_misc/WSLInterop-late ]]; then
    windows=true
fi

case "${ARTISAN_DEV_PROFILE:-debug}" in
    debug) profile=dev profile_dir=debug ;;
    release) profile=release profile_dir=release ;;
    *) die "ARTISAN_DEV_PROFILE is debug or release, not '$ARTISAN_DEV_PROFILE'" ;;
esac

# ---- building ---------------------------------------------------------------

# The default dev shell caps Cargo at two jobs for interactive use.
unset CARGO_BUILD_JOBS CARGO_TARGET_DIR CARGO_BUILD_TARGET

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

# Replaces a file by rename, so a running copy keeps its old inode.
put() {
    mkdir -p "$(dirname "$2")"
    cp -f "$1" "$2.next"
    mv -f "$2.next" "$2"
}

# ---- the dev Forge ----------------------------------------------------------

# The pid in forge.pid if it is the dev `ae` (replaced binaries read "(deleted)").
forge_pid() {
    local pid exe
    pid="$(cat "$pid_file" 2>/dev/null)" || return 1
    [[ "$pid" =~ ^[0-9]+$ ]] || return 1
    exe="$(readlink "/proc/$pid/exe" 2>/dev/null)" || return 1
    [[ "$exe" == "$ae" || "$exe" == "$ae (deleted)" ]] || return 1
    echo "$pid"
}

# Stops a process and waits for it: TERM, then KILL after <seconds>.
stop_pid() {
    local pid="$1" seconds="$2"
    kill -TERM "$pid" 2>/dev/null || return 0
    if ! timeout "$seconds" tail --pid="$pid" -s 0.1 -f /dev/null; then
        say "pid $pid did not stop within $seconds s; killing it"
        kill -KILL "$pid" 2>/dev/null || true
        timeout 5 tail --pid="$pid" -s 0.1 -f /dev/null || true
    fi
}

stop_forge() {
    local pid
    if pid="$(forge_pid)"; then
        say "stopping the dev Forge (pid $pid)"
        stop_pid "$pid" 30
    fi
    rm -f "$pid_file"
}

start_forge() {
    local pid host port
    mkdir -p "$ARTISAN_FORGE_DATA"
    if [[ -f "$forge_log" ]] && (($(stat -c %s "$forge_log") > 16 * 1024 * 1024)); then
        mv -f "$forge_log" "$forge_log.1"
    fi
    # Its own session: no terminal hangup reaches it, and it outlives this run.
    setsid "$ae" forge serve >> "$forge_log" 2>&1 < /dev/null &
    pid=$!
    echo "$pid" > "$pid_file"
    host="${ARTISAN_FORGE_LISTEN%:*}"
    port="${ARTISAN_FORGE_LISTEN##*:}"
    for _ in $(seq 100); do
        if ! kill -0 "$pid" 2> /dev/null; then
            tail -n 20 "$forge_log" >&2
            die "the dev Forge exited while starting; log: $forge_log"
        fi
        if (exec 3<> "/dev/tcp/$host/$port") 2> /dev/null; then
            say "dev Forge on $ARTISAN_FORGE_LISTEN (pid $pid, data $ARTISAN_FORGE_DATA, log $forge_log)"
            return
        fi
        sleep 0.1
    done
    die "the dev Forge (pid $pid) is not listening on $ARTISAN_FORGE_LISTEN after 10 s; log: $forge_log"
}

# The bash that `wsl.exe -- …` starts for the dev Windows Editor sources this
# (BASH_ENV, passed through WSLENV by launch.cmd).
write_wsl_env() {
    mkdir -p "$ARTISAN_HOME"
    {
        echo "# Written by \`nix run .#dev\`: makes \`wsl.exe -- ae …\` from the dev Editor reach the dev Forge."
        echo "unset BASH_ENV"
        # shellcheck disable=SC2016 # $PATH expands when the file is sourced.
        printf 'export PATH=%q:"$PATH"\n' "$bin"
        printf 'export ARTISAN_HOME=%q ARTISAN_FORGE_DATA=%q ARTISAN_FORGE_LISTEN=%q\n' \
            "$ARTISAN_HOME" "$ARTISAN_FORGE_DATA" "$ARTISAN_FORGE_LISTEN"
    } > "$ARTISAN_HOME/wsl-env.sh"
}

# ---- the Linux Editor -------------------------------------------------------

linux_editor_pids() {
    local proc exe
    for proc in /proc/[0-9]*; do
        exe="$(readlink "$proc/exe" 2>/dev/null)" || continue
        if [[ "$exe" == "$editor" || "$exe" == "$editor (deleted)" ]]; then echo "${proc#/proc/}"; fi
    done
}

stop_linux_editor() {
    local pid
    for pid in $(linux_editor_pids); do
        say "closing the dev Editor (pid $pid)"
        stop_pid "$pid" 10
    done
}

launch_linux_editor() {
    mkdir -p "$ARTISAN_HOME/editor"
    local library_path="$ARTISAN_DEV_GRAPHICS_LIBRARY_PATH" dir
    for dir in /usr/lib/wsl/lib /run/opengl-driver/lib; do
        if [[ -d "$dir" ]]; then library_path="$library_path:$dir"; fi
    done
    # `ae` on PATH is the dev one, for "pair with this computer".
    ARTISAN_EDITOR_DATA="$ARTISAN_HOME/editor" PATH="$bin:$PATH" \
        LD_LIBRARY_PATH="$library_path${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" \
        setsid "$editor" >> "$ARTISAN_HOME/editor.log" 2>&1 < /dev/null &
    say "dev Editor started (pid $!, data $ARTISAN_HOME/editor, log $ARTISAN_HOME/editor.log)"
}

# ---- the Windows Editor (inside WSL) ----------------------------------------

windows_setup() {
    local cmd system_root local_app_data
    cmd="$(command -v cmd.exe || true)"
    for candidate in /mnt/c/Windows/System32/cmd.exe /mnt/c/WINDOWS/system32/cmd.exe; do
        if [[ -z "$cmd" && -x "$candidate" ]]; then cmd="$candidate"; fi
    done
    [[ -n "$cmd" ]] || die "can't find cmd.exe; is WSL interop enabled?"
    # Windows programs run from a Windows directory: cmd.exe refuses a
    # \\wsl.localhost working directory.
    windows_cwd="$(dirname "$cmd")"
    system_root="$(cd "$windows_cwd" && "$cmd" /d /c 'echo %SystemRoot%' | tr -d '\r')"
    local_app_data="$(cd "$windows_cwd" && "$cmd" /d /c 'echo %LOCALAPPDATA%' | tr -d '\r')"
    [[ -n "$system_root" && -n "$local_app_data" && "$local_app_data" != *%* ]] || die "can't read %LOCALAPPDATA% through cmd.exe"
    windows_dir_win="$local_app_data\\Artisan Dev"
    windows_dir="$(wslpath -u "$windows_dir_win")"
    powershell="$(wslpath -u "$system_root")/System32/WindowsPowerShell/v1.0/powershell.exe"
    explorer="$(wslpath -u "$system_root")/explorer.exe"
}

# Closes Editors running from the dev directory (WM_CLOSE, then forced after 10 s).
stop_windows_editor() {
    local script
    # shellcheck disable=SC2016 # PowerShell, not bash.
    script='$dir = $env:ARTISAN_DEV_WINDOWS_DIR + "\"
$p = @(Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -and $_.ExecutablePath.StartsWith($dir, "OrdinalIgnoreCase") })
foreach ($x in $p) { Write-Host "dev: closing the dev Editor (pid $($x.ProcessId))"; taskkill.exe /PID $x.ProcessId | Out-Null }
foreach ($x in $p) { Wait-Process -Id $x.ProcessId -Timeout 10 -ErrorAction SilentlyContinue }
foreach ($x in $p) { Stop-Process -Id $x.ProcessId -Force -ErrorAction SilentlyContinue }'
    (cd "$windows_cwd" && ARTISAN_DEV_WINDOWS_DIR="$windows_dir_win" WSLENV="${WSLENV:+$WSLENV:}ARTISAN_DEV_WINDOWS_DIR" \
        "$powershell" -NoProfile -NonInteractive -Command "$script" | tr -d '\r')
}

install_windows_editor() {
    mkdir -p "$windows_dir/editor"
    say "copying the Editor to $windows_dir_win"
    put "$windows_out/artisan-editor.exe" "$windows_dir/artisan-editor.exe"
    write_wsl_env
    printf '%s\r\n' \
        '@echo off' \
        'rem Starts the dev Editor. Written by nix run .#dev.' \
        'set "ARTISAN_EDITOR_DATA=%~dp0editor"' \
        "set \"BASH_ENV=$ARTISAN_HOME/wsl-env.sh\"" \
        'if defined WSLENV (set "WSLENV=%WSLENV%:BASH_ENV") else (set "WSLENV=BASH_ENV")' \
        'start "" "%~dp0artisan-editor.exe" %*' > "$windows_dir/launch.cmd"
}

# Explorer starts it, outside the job WSL interop puts this run's Windows
# processes in, so the Editor outlives this terminal.
launch_windows_editor() {
    (cd "$windows_cwd" && "$explorer" "$windows_dir_win\\launch.cmd") || true
    say "dev Editor started from $windows_dir_win (data in editor\\ there); pair it with this distribution from its Home screen"
}

stop_editor() {
    if [[ "$windows" == true ]]; then
        windows_setup
        stop_windows_editor
    else
        stop_linux_editor
    fi
}

# ---- commands ---------------------------------------------------------------

build_ae_and_linux_editor() {
    local args=(-p ae --bin ae)
    if [[ "$1" == true && "$windows" == false ]]; then args+=(-p editor --bin artisan-editor); fi
    cargo_build cargo-linux linux "${args[@]}"
}

install_ae() {
    put "$linux_out/ae" "$ae"
    write_wsl_env
}

relaunch_editor() {
    if [[ "$windows" == true ]]; then
        windows_setup
        stop_windows_editor
        install_windows_editor
        launch_windows_editor
    else
        stop_linux_editor
        put "$linux_out/artisan-editor" "$editor"
        launch_linux_editor
    fi
}

case "$command" in
    run)
        # Everything builds before anything running is touched.
        build_ae_and_linux_editor true
        if [[ "$windows" == true ]]; then cargo_build cargo-windows windows -p editor --bin artisan-editor; fi
        stop_forge
        install_ae
        start_forge
        relaunch_editor
        ;;
    forge)
        build_ae_and_linux_editor false
        stop_forge
        install_ae
        start_forge
        ;;
    editor)
        if [[ "$windows" == true ]]; then
            cargo_build cargo-windows windows -p editor --bin artisan-editor
        else
            build_ae_and_linux_editor true
            install_ae
        fi
        relaunch_editor
        ;;
    stop)
        stop_editor
        stop_forge
        say "stopped"
        ;;
    import)
        from="${from:-${XDG_DATA_HOME:-$HOME/.local/share}/Artisan Street Dev/data}"
        [[ -f "$from/forge.sqlite3" ]] || die "no old Forge database at $from/forge.sqlite3 (pass --from DIR)"
        build_ae_and_linux_editor false
        stop_forge
        install_ae
        say "importing $from into $ARTISAN_FORGE_DATA (the old database is only read)"
        "$ae" forge import --from "$from"
        start_forge
        ;;
esac
