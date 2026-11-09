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
  read -r width height < <(png_size "$reference/$name.png" | awk -v s="$scale" '{printf "%.4f %.4f\n", $1/s, $2/s}')
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
  echo "== aqua $name"
  run_aqua "$name" "$route" "$here/fixtures/$fixture" "$scale" "$here/scripts/$script" "$work" || echo "FAILED $name" >&2
  rm -rf "$work"
done <"$here/screens.txt"
