#!/usr/bin/env bash
set -euo pipefail
out=$(mkdir -p "${1:?usage: capture.sh OUT_DIR [screen...]}" && cd "$1" && pwd)
shift
here=$(cd "$(dirname "$0")" && pwd)
: "${AQUA_BIN:?AQUA_BIN is the editor-aqua binary built with --features capture}"
reference=${AQUA_REFERENCE:-"$here/baselines/aqua"}

png_size() {
  python3 -I -c 'import struct,sys;d=open(sys.argv[1],"rb").read(24);print(*struct.unpack(">II",d[16:24]))' "$1"
}

run_aqua() {
  local name=$1 route=$2 fixture=$3 scale=$4 script=$5 work=$6
  [ -f "$reference/$name.png" ] || { echo "no baseline $reference/$name.png" >&2; return 1; }
  local size
  size=$(png_size "$reference/$name.png") || return 1
  read -r width height < <(awk -v s="$scale" '{printf "%.4f %.4f\n", $1/s, $2/s}' <<<"$size") || return 1
  sed -e 's/^capture @\(.*\)/capture '"$name"'\1.png/' "$script" >"$work/script.txt" || return 1
  TZ=UTC LC_ALL=C LANG=C ARTISAN_EDITOR_SCRIPT="$work/script.txt" ARTISAN_AQUA_CAPTURE_FIXTURE="$fixture" \
    ARTISAN_AQUA_CAPTURE_ROUTE="$route" ARTISAN_AQUA_WIDTH="$width" ARTISAN_AQUA_HEIGHT="$height" \
    ARTISAN_AQUA_SCALE="$scale" ARTISAN_AQUA_CAPTURE_DIR="$out" timeout 300 "$AQUA_BIN" >"$work/aqua.log" 2>&1 \
    || { tail -5 "$work/aqua.log" >&2; return 1; }
}

for wanted in "$@"; do
  cut -d'|' -f1 "$here/screens.txt" | grep -qx -- "$wanted" || { echo "unknown screen $wanted" >&2; exit 1; }
done

failed=()
captured=0
while IFS='|' read -r name route fixture scale script; do
  case $name in ''|'#'*) continue ;; esac
  if [ "$#" -gt 0 ]; then printf '%s\n' "$@" | grep -qx "$name" || continue; fi
  work=$(mktemp -d)
  echo "== aqua $name"
  rm -f "$out/$name.png"
  if run_aqua "$name" "$route" "$here/fixtures/$fixture" "$scale" "$here/scripts/$script" "$work" && [ -s "$out/$name.png" ]; then
    captured=$((captured + 1))
  else
    echo "FAILED $name" >&2
    failed+=("$name")
  fi
  rm -rf "$work"
done <"$here/screens.txt"

if [ "${#failed[@]}" -gt 0 ]; then
  echo "capture failed for: ${failed[*]}" >&2
  exit 1
fi
if [ "$captured" -eq 0 ]; then
  echo "no screen matched: $*" >&2
  exit 1
fi
