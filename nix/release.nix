# The unsigned release: what `ae install --from <dir|url>` reads
# (crates/ae/src/install/release.rs), minus `release.json.sig`.
#
#   release.json                          {"version", "artifacts": [{os, arch, file, sha256}]}
#   artisan-<v>-linux-x86_64.tar.gz       ae, artisan-editor at the top level
#   artisan-<v>-windows-x86_64.zip        ae.exe, artisan-editor.exe
#   install.sh, install.ps1               the bootstrap installers
#
# `scripts/release` builds this, copies it out and signs release.json with a
# key that never enters the store. Archives are byte-reproducible.
{
  pkgs,
  version,
  linux,
  windows,
}:
let
  linuxName = "artisan-${version}-linux-x86_64.tar.gz";
  windowsName = "artisan-${version}-windows-x86_64.zip";
in
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
    mkdir -p "$out" linux windows

    # Linux binaries run on ordinary glibc distributions (WSL Ubuntu above
    # all): the system loader and the system's libraries, not the store's.
    install -m755 ${linux.ae}/bin/ae ${linux.editor}/bin/artisan-editor linux/
    patchelf --set-interpreter /lib64/ld-linux-x86-64.so.2 --remove-rpath linux/ae linux/artisan-editor
    newest="$(objdump -T linux/ae linux/artisan-editor | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -n1)"
    echo "the Linux binaries need glibc ''${newest#GLIBC_} or newer"
    if [ "$(printf '%s\n' "$newest" GLIBC_2.39 | sort -V | tail -n1)" != GLIBC_2.39 ]; then
      echo "error: $newest is newer than Ubuntu 24.04's glibc 2.39" >&2
      exit 1
    fi
    tar --sort=name --mtime=@315532800 --owner=0 --group=0 --numeric-owner --mode=0755 \
      -C linux -cf - ae artisan-editor | gzip -9n > "$out/${linuxName}"

    install -m755 ${windows.ae}/bin/ae.exe ${windows.editor}/bin/artisan-editor.exe windows/
    touch -d @315532800 windows/*
    (cd windows && TZ=UTC zip -X -9 -q "$out/${windowsName}" ae.exe artisan-editor.exe)

    # One artifact per line: install.sh reads it without a JSON parser.
    artifact() {
      jq -cn --arg os "$1" --arg file "$2" --arg sha256 "$(sha256sum "$out/$2" | cut -d' ' -f1)" \
        '{os: $os, arch: "x86_64", file: $file, sha256: $sha256}'
    }
    {
      printf '{"version":"%s","artifacts":[\n' ${version}
      artifact linux ${linuxName}
      printf ',\n'
      artifact windows ${windowsName}
      printf ']}\n'
    } > "$out/release.json"
    jq -e . "$out/release.json" > /dev/null

    # The bootstrap installers default to the feed `ae` itself uses.
    feed="$(sed -n 's/^pub const DEFAULT_FEED: &str = "\(.*\)";$/\1/p' ${../crates/ae/src/install/release.rs})"
    for script in ${../scripts/install.sh} ${../scripts/install.ps1}; do
      if ! grep -qF "$feed" "$script"; then
        echo "error: $script does not default to $feed" >&2
        exit 1
      fi
    done
    install -m755 ${../scripts/install.sh} "$out/install.sh"
    install -m644 ${../scripts/install.ps1} "$out/install.ps1"
  ''
