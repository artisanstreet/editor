# The unsigned release: what `ae install` reads (crates/ae/src/release.rs),
# minus `release.json.sig`. One archive per app and platform:
#
#   release.json                            {"version", "artifacts": [{app, os, arch, file, sha256}]}
#   ae-<v>-linux-x86_64.tar.gz              ae
#   forge-<v>-linux-x86_64.tar.gz           forge
#   editor-<v>-linux-x86_64.tar.gz          artisan-editor, notices
#   ae-<v>-windows-x86_64.zip               ae.exe          (and so on)
#   install.sh, install.ps1                 the bootstrap installers (fetch ae, run `ae install`)
#
# `scripts/release` builds this, copies it out and signs release.json with a
# key that never enters the store. Archives are byte-reproducible.
{
  pkgs,
  version,
  linux,
  windows,
}:
pkgs.runCommand "artisan-release-${version}"
  {
    nativeBuildInputs = with pkgs; [
      patchelf
      gnutar
      gzip
      zip
      jq
      binutils
    ];
  }
  ''
    mkdir -p "$out"
    artifacts=()

    # stage <dir> <app>: the licence, plus the Editor's notices.
    stage() {
      mkdir -p "$1"
      install -m644 ${../LICENSE.md} "$1/LICENSE.md"
      if [ "$2" = editor ]; then
        install -m644 ${../vendor/highlighting/NOTICE.txt} "$1/HIGHLIGHTING-NOTICES.txt"
        cp -R ${../crates/editor-assets/licenses} "$1/notices"
        chmod -R u+w "$1/notices"
      fi
    }

    # artifact <app> <os> <file>
    artifact() {
      artifacts+=("$(jq -cn --arg app "$1" --arg os "$2" --arg file "$3" \
        --arg sha256 "$(sha256sum "$out/$3" | cut -d' ' -f1)" \
        '{app: $app, os: $os, arch: "x86_64", file: $file, sha256: $sha256}')")
    }

    # Linux binaries run on ordinary glibc distributions (WSL Ubuntu above
    # all): the system loader and the system's libraries, not the store's.
    linux() {
      local app="$1" binary="$2" dir="linux-$1" file="$1-${version}-linux-x86_64.tar.gz"
      stage "$dir" "$app"
      install -m755 "$binary" "$dir/"
      local name
      name="$(basename "$binary")"
      patchelf --set-interpreter /lib64/ld-linux-x86-64.so.2 --remove-rpath "$dir/$name"
      newest="$(objdump -T "$dir/$name" | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -n1)"
      echo "$name needs glibc ''${newest#GLIBC_} or newer"
      if [ "$(printf '%s\n' "$newest" GLIBC_2.39 | sort -V | tail -n1)" != GLIBC_2.39 ]; then
        echo "error: $newest is newer than Ubuntu 24.04's glibc 2.39" >&2
        exit 1
      fi
      (cd "$dir" && tar --sort=name --mtime=@315532800 --owner=0 --group=0 --numeric-owner --mode=u+rwX,go+rX \
        -cf - * | gzip -9n > "$out/$file")
      artifact "$app" linux "$file"
    }

    windows() {
      local app="$1" binary="$2" name="''${3:-$(basename "$2")}" dir="windows-$1" file="$1-${version}-windows-x86_64.zip"
      stage "$dir" "$app"
      install -m755 "$binary" "$dir/$name"
      find "$dir" -exec touch -d @315532800 {} +
      (cd "$dir" && TZ=UTC zip -X -9 -q -r "$out/$file" .)
      artifact "$app" windows "$file"
    }

    linux ae ${linux.ae}/bin/ae
    linux forge ${linux.forge}/bin/forge
    linux editor ${linux.editor}/bin/artisan-editor
    windows ae ${windows.ae}/bin/ae.exe
    windows forge ${windows.forge}/bin/forge.exe
    windows editor ${windows.editor}/bin/artisan-editor.exe "Artisan Editor.exe"

    # One artifact per line: install.sh reads it without a JSON parser.
    {
      printf '{"version":"%s","artifacts":[\n' ${version}
      first=1
      for line in "''${artifacts[@]}"; do
        if [ "$first" = 1 ]; then first=0; else printf ',\n'; fi
        printf '%s' "$line"
      done
      printf '\n]}\n'
    } > "$out/release.json"
    jq -e . "$out/release.json" > /dev/null

    # The bootstrap installers default to the feed `ae` itself uses.
    feed="$(sed -n '/^pub const DEFAULT_FEED: &str =/,/;/p' ${../crates/ae/src/release.rs} | sed -n 's/.*"\([^"]*\)";.*/\1/p')"
    [ -n "$feed" ] || { echo "error: release feed extraction failed" >&2; exit 1; }
    for script in ${../scripts/install.sh} ${../scripts/install.ps1}; do
      if ! grep -qF "$feed" "$script"; then
        echo "error: $script does not default to $feed" >&2
        exit 1
      fi
    done
    install -m755 ${../scripts/install.sh} "$out/install.sh"
    install -m644 ${../scripts/install.ps1} "$out/install.ps1"
  ''
