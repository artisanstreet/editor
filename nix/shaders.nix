{ pkgs }:
let
  inherit (pkgs) lib;
  sdk = pkgs.stdenvNoCC.mkDerivation {
    pname = "windows-sdk-fxc";
    version = "10.0.26100.1";
    src = pkgs.fetchurl {
      url = "https://api.nuget.org/v3-flatcontainer/microsoft.windows.sdk.cpp/10.0.26100.1/microsoft.windows.sdk.cpp.10.0.26100.1.nupkg";
      hash = "sha256-tHMKRnqPKRRfwBNrKz9iZ2fphfaqbjLxNSlTw41v9dI=";
    };
    licenseSource = pkgs.fetchurl {
      url = "https://download.microsoft.com/download/0/F/F/0FF2B061-47DD-4F55-89B6-FD1D8C44F14D/sdk_license.rtf";
      hash = "sha256-3QfrF44AxrukFIRX/AD/d81Ih+tSHVBBhv5ZyeyLvmI=";
    };
    nativeBuildInputs = [ pkgs.unzip ];
    dontUnpack = true;
    dontBuild = true;
    installPhase = ''
      runHook preInstall
      mkdir -p "$out/libexec" "$out/share/windows-sdk-fxc"
      for name in fxc.exe d3dcompiler_47.dll; do
        unzip -p "$src" "c/bin/10.0.26100.0/x64/$name" > "$out/libexec/$name"
      done
      install -m644 "$src" "$out/share/windows-sdk-fxc/Microsoft.Windows.SDK.CPP.10.0.26100.1.nupkg"
      install -m644 "$licenseSource" "$out/share/windows-sdk-fxc/sdk_license.rtf"
      unzip -p "$src" Microsoft.Windows.SDK.CPP.nuspec > "$out/share/windows-sdk-fxc/Microsoft.Windows.SDK.CPP.nuspec"
      runHook postInstall
    '';
    meta = {
      description = "Microsoft Windows SDK FXC and its matching native shader compiler DLL";
      homepage = "https://www.nuget.org/packages/Microsoft.Windows.SDK.CPP/10.0.26100.1";
      license = lib.licenses.unfree;
      sourceProvenance = [ lib.sourceTypes.binaryNativeCode ];
      platforms = [ "x86_64-linux" ];
    };
  };
  wine = pkgs.wine64Packages.minimal;
  fxc = pkgs.writeShellApplication {
    name = "aqua-fxc";
    runtimeInputs = [ pkgs.coreutils ];
    text = ''
      export WINEARCH=win64
      export WINEDEBUG=-all
      export WINEDLLOVERRIDES="d3dcompiler_47=n;winemenubuilder.exe=d"
      unset DISPLAY WAYLAND_DISPLAY
      owned_prefix=false
      if [[ -n "''${AQUA_FXC_WINEPREFIX:-}" ]]; then
        export WINEPREFIX="$AQUA_FXC_WINEPREFIX"
      else
        export WINEPREFIX
        WINEPREFIX="$(mktemp -d "''${TMPDIR:-/tmp}/aqua-fxc.XXXXXX")"
        owned_prefix=true
      fi
      cleanup() {
        if [[ "$owned_prefix" == true ]]; then
          ${wine}/bin/wineserver -k || true
          ${wine}/bin/wineserver -w || true
          rm -rf "$WINEPREFIX"
        fi
      }
      trap cleanup EXIT
      case "''${1:-}" in
        --initialize)
          mkdir -p "$WINEPREFIX"
          if ! ${wine}/bin/wineboot --init > "$WINEPREFIX/initialize.log" 2>&1; then
            cat "$WINEPREFIX/initialize.log" >&2
            exit 1
          fi
          exit
          ;;
        --shutdown)
          ${wine}/bin/wineserver -k
          ${wine}/bin/wineserver -w
          exit
          ;;
      esac
      windows_path() {
        local path
        path="$(realpath -m "$1")"
        printf 'Z:%s' "''${path//\//\\}"
      }
      arguments=()
      while (($#)); do
        case "$1" in
          -I|-Fo)
            arguments+=("$1" "$(windows_path "$2")")
            shift 2
            ;;
          -T|-E|-D)
            arguments+=("$1" "$2")
            shift 2
            ;;
          -nologo|-Zpr|-O3)
            arguments+=("$1")
            shift
            ;;
          -*)
            echo "Unsupported Aqua FXC option: $1" >&2
            exit 2
            ;;
          *)
            arguments+=("$(windows_path "$1")")
            shift
            ;;
        esac
      done
      ${wine}/bin/wine ${sdk}/libexec/fxc.exe "''${arguments[@]}"
    '';
  };
in
{
  inherit fxc;
  prepare = ''
    export AQUA_FXC_WINEPREFIX="$(mktemp -d "''${TMPDIR:-/tmp}/aqua-fxc.XXXXXX")"
    ${fxc}/bin/aqua-fxc --initialize
  '';
  cleanup = ''
    ${fxc}/bin/aqua-fxc --shutdown
    rm -rf "$AQUA_FXC_WINEPREFIX"
  '';
}
