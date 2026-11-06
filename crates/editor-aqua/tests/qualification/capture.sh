#!/usr/bin/env bash
set -euo pipefail
side=${1:?usage: capture.sh gpui|aqua OUT_DIR [screen...]}
out=$(mkdir -p "${2:?usage: capture.sh gpui|aqua OUT_DIR [screen...]}" && cd "$2" && pwd)
shift 2
here=$(cd "$(dirname "$0")" && pwd)
: "${AQUA_BIN:?AQUA_BIN is the editor-aqua binary built with --features capture}"
[ "$side" = aqua ] || : "${GPUI_BIN:?GPUI_BIN is the artisan-editor binary built with --features capture}"
display=${DISPLAY:-:0}
host=mch_capture0000000000000000

png_size() {
  python3 -I -c 'import struct,sys;d=open(sys.argv[1],"rb").read(24);print(*struct.unpack(">II",d[16:24]))' "$1"
}

run_gpui() {
  local name=$1 route=$2 fixture=$3 scale=$4 script=$5 work=$6
  mkdir -p "$work/editor" "$work/xdg/artisan-street/ae"
  printf '#!/bin/sh\necho "{\\"enrolled\\":true,\\"machine_id\\":\\"%s\\",\\"account_id\\":\\"acct_capture0000000000000000\\"}"\n' "$host" >"$work/xdg/artisan-street/ae/ae"
  chmod +x "$work/xdg/artisan-street/ae/ae"
  mkfifo "$work/hold"
  sleep infinity >"$work/hold" &
  local holder=$!
  ARTISAN_AQUA_SERVE_EDITOR_DIR="$work/editor" ARTISAN_AQUA_CAPTURE_FIXTURE="$fixture" \
    "$AQUA_BIN" <"$work/hold" >"$work/serve.log" 2>&1 &
  local server=$!
  for _ in $(seq 1 100); do grep -q '^serving' "$work/serve.log" 2>/dev/null && break; sleep 0.2; done
  grep -q '^serving' "$work/serve.log" || { echo "forge did not start for $name" >&2; kill "$holder" "$server" 2>/dev/null; return 1; }
  sed -e 's/^settle.*/wait 3000/' -e "s|^capture @\(.*\)|capture $out/$name\1.png|" "$script" >"$work/script.txt"
  local link
  case $route in
    thread:*) link="artisan://thread/$host/${route#thread:}" ;;
    new-thread) link="artisan://new/$host" ;;
    settings:*) link="artisan://settings/${route#settings:}" ;;
    marketplace) link="artisan://marketplace" ;;
    usage) link="artisan://usage" ;;
  esac
  local status=0
  DISPLAY=$display ARTISAN_EDITOR_DATA="$work/editor" ARTISAN_EDITOR_SCRIPT="$work/script.txt" \
    XDG_DATA_HOME="$work/xdg" XDG_CONFIG_HOME="$work/xdg" TZ=UTC LC_ALL=C LANG=C \
    GPUI_X11_SCALE_FACTOR="$scale" timeout 180 "$GPUI_BIN" "$link" >"$work/gpui.log" 2>&1 || status=$?
  kill "$holder" "$server" 2>/dev/null || true
  wait "$server" 2>/dev/null || true
  [ "$status" = 0 ] || { tail -5 "$work/gpui.log" >&2; return "$status"; }
}

run_aqua() {
  local name=$1 route=$2 fixture=$3 scale=$4 script=$5 work=$6
  read -r width height < <(png_size "${GPUI_REFERENCE:?GPUI_REFERENCE is the directory of GPUI captures}/$name.png" | awk -v s="$scale" '{printf "%.4f %.4f\n", $1/s, $2/s}')
  sed -e 's/^capture @\(.*\)/capture '"$name"'\1.png/' "$script" >"$work/script.txt"
  TZ=UTC LC_ALL=C LANG=C ARTISAN_EDITOR_SCRIPT="$work/script.txt" ARTISAN_AQUA_CAPTURE_FIXTURE="$fixture" \
    ARTISAN_AQUA_CAPTURE_ROUTE="$route" ARTISAN_AQUA_WIDTH="$width" ARTISAN_AQUA_HEIGHT="$height" \
    ARTISAN_AQUA_SCALE="$scale" ARTISAN_AQUA_CAPTURE_DIR="$out" timeout 300 "$AQUA_BIN" >"$work/aqua.log" 2>&1 \
    || { tail -5 "$work/aqua.log" >&2; return 1; }
}

while IFS='|' read -r name route fixture scale script; do
  case $name in ''|'#'*) continue ;; esac
  if [ "$#" -gt 0 ]; then printf '%s\n' "$@" | grep -qx "$name" || continue; fi
  work=$(mktemp -d)
  echo "== $side $name"
  "run_$side" "$name" "$route" "$here/fixtures/$fixture" "$scale" "$here/scripts/$script" "$work" || echo "FAILED $name" >&2
  rm -rf "$work"
done <"$here/screens.txt"
