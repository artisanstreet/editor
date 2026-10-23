# Packages, checks and shells of the workspace.
#
# Three shipped apps: `ae` (crates/ae: installs and manages the others),
# `forge` (crates/forge) and `artisan-editor` (crates/editor), built for
# Linux and cross-built for Windows (x86_64-pc-windows-gnu, MinGW-w64).
# `release` lays them out the way `ae install` expects (nix/release.nix);
# signing is `scripts/release`.
{
  self,
  pkgs,
  windowsPkgs,
  crane,
}:
let
  inherit (pkgs) lib;
  manifest = builtins.fromTOML (builtins.readFile ../Cargo.toml);
  inherit (manifest.workspace.package) version;
  rust = pkgs.rust-bin.fromRustupToolchainFile ../rust-toolchain.toml;
  craneLib = (crane.mkLib pkgs).overrideToolchain rust;

  # The crates plus what they embed (skills/) and Cargo's configuration.
  roots = [
    "crates"
    "vendor/highlighting"
    "skills"
    ".cargo"
    ".config"
  ];
  topFiles = [
    "Cargo.toml"
    "Cargo.lock"
    "clippy.toml"
    "rust-toolchain.toml"
    "rustfmt.toml"
  ];
  src = lib.cleanSourceWith {
    src = ../.;
    filter =
      path: type:
      let
        relative = lib.removePrefix (toString ../. + "/") (toString path);
        under =
          root:
          relative == root
          || lib.hasPrefix "${root}/" relative
          || (type == "directory" && lib.hasPrefix "${relative}/" root);
        parts = lib.splitString "/" relative;
      in
      (builtins.elem relative topFiles || builtins.any under roots)
      && !(builtins.any (
        part:
        builtins.elem part [
          "target"
          ".git"
        ]
      ) parts);
  };

  # Libraries the Linux Editor links or loads (GPUI: Wayland, X11/xcb,
  # xkbcommon, Vulkan, fontconfig/freetype).
  libraries = with pkgs; [
    fontconfig
    freetype
    libxkbcommon
    wayland
    libX11
    libXcursor
    libXi
    libXrandr
    libxcb
    vulkan-loader
    libGL
    openssl
    sqlite
  ];
  graphicsLibraryPath = lib.makeLibraryPath (libraries ++ [ pkgs.mesa ]);
  nativeTools = with pkgs; [
    git
    pkg-config
    clang
    cmake
    perl
  ];
  # Vendor build scripts can copy immutable inputs into their OUT_DIR.
  # Limit permission repair to build-script outputs; never scan object files,
  # incremental caches or the rest of the restored Cargo target artifacts.
  writableTarget = ''
    reform_target="''${CARGO_TARGET_DIR:-target}"
    for reform_output in "$reform_target"/*/build/*/out "$reform_target"/*/*/build/*/out; do
      if [ -d "$reform_output" ]; then
        find "$reform_output" -type f ! -perm -u=w -exec chmod u+w {} +
      fi
    done
  '';
  rerunnableBuild = {
    preBuild = writableTarget;
    cargoBuildCommand = "cargoWithProfile build";
  };

  # Commit for `ae --version` (the sandbox has no .git).
  commit =
    let
      rev = self.rev or self.dirtyRev or null;
    in
    if rev == null then "unknown" else builtins.substring 0 10 rev;

  platforms = {
    linux = rec {
      target = pkgs.stdenv.hostPlatform.rust.rustcTarget;
      exe = name: name;
      craneLib = (crane.mkLib pkgs).overrideToolchain (
        p: p.rust-bin.fromRustupToolchainFile ../rust-toolchain.toml
      );
      build = craneLib.buildPackage;
      deps = craneLib.buildDepsOnly;
      environment = {
        nativeBuildInputs = nativeTools;
        buildInputs = libraries;
        LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
      };
      # mold, wrapped by nixpkgs like the default linker, for the dev loop.
      fastLinker = environment // {
        nativeBuildInputs = nativeTools ++ [ pkgs.mold ];
        NIX_CFLAGS_LINK = "-fuse-ld=mold";
      };
    };
    windows = rec {
      target = "x86_64-pc-windows-gnu";
      exe = name: "${name}.exe";
      craneLib = (crane.mkLib windowsPkgs).overrideToolchain (
        p: p.rust-bin.fromRustupToolchainFile ../rust-toolchain.toml
      );
      build = attrs: windowsPkgs.callPackage ({ ... }: craneLib.buildPackage attrs) { };
      deps = attrs: windowsPkgs.callPackage ({ ... }: craneLib.buildDepsOnly attrs) { };
      # nixpkgs builds MinGW GCC with the mcf thread model: C code using
      # thread-locals (mimalloc) pulls in emutls, which needs mcfgthread.
      # Link its static archive and the NT libraries it calls, so every .exe
      # imports only Windows system DLLs. This replaces the target's
      # rustflags from .cargo/config.toml, so it repeats them.
      rustflags = [
        "-C target-cpu=x86-64-v3"
        "-C target-feature=+crt-static"
        "-C link-arg=-l:libmcfgthread.a"
        "-C link-arg=-lntdll"
        "-C link-arg=-lkernel32"
      ];
      environment =
        let
          cc = windowsPkgs.stdenv.cc;
        in
        {
          depsBuildBuild = [ pkgs.stdenv.cc ];
          # Compile the native D3D11 HLSL on the Linux build host.
          nativeBuildInputs = nativeTools ++ [ pkgs.vkd3d ];
          CARGO_BUILD_TARGET = target;
          buildInputs = [
            windowsPkgs.windows.pthreads
            windowsPkgs.windows.mcfgthreads
          ];
          CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS = lib.concatStringsSep " " rustflags;
          CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = "${cc}/bin/${cc.targetPrefix}cc";
          TARGET_CC = "${cc}/bin/${cc.targetPrefix}cc";
          TARGET_CXX = "${cc}/bin/${cc.targetPrefix}c++";
          TARGET_AR = "${cc.bintools.bintools}/bin/${cc.targetPrefix}ar";
          # The Editor's build script compiles its icon resource.
          TARGET_WINDRES = "${cc.bintools.bintools}/bin/${cc.targetPrefix}windres";
          HOST_CC = "${pkgs.stdenv.cc}/bin/cc";
          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
        };
      # Keep the release GNU linker in the development shell too. With the
      # two windows-targets import-library versions in this dependency graph,
      # lld places some ntdll imports after their descriptor's terminator.
      # Linking succeeds, but Windows leaves those call targets unbound and
      # the Editor crashes before main. The same inputs link correctly here.
      fastLinker = environment;
    };
  };

  # One shipped binary for one platform, in Cargo's `release` profile
  # (Cargo.toml). Each binary compiles only its own dependency graph, so
  # `ae` never waits on GPUI.
  binary =
    {
      platform,
      package,
      name,
    }:
    let
      host = platforms.${platform};
      common =
        host.environment
        // rerunnableBuild
        // {
          inherit src version;
          pname = "${name}-${platform}";
          strictDeps = true;
          CARGO_PROFILE = "release";
          cargoExtraArgs = "--locked --offline -p ${package} --bin ${name}";
          doCheck = false;
        };
      profileDirectory = "target/${lib.optionalString (platform == "windows") "${host.target}/"}release";
    in
    host.build (
      common
      // {
        cargoArtifacts = host.deps (
          common
          // {
            pname = "${name}-${platform}-dependencies";
            # Only `cargo build` reads the cache; crane's extra `check` pass
            # costs minutes.
            cargoCheckCommand = "true";
          }
        );
        # The dependencies are cached; what is left ends in one fat-LTO link
        # (the Editor's peaks near 5.5 GB), so one job at a time.
        CARGO_BUILD_JOBS = 1;
        # Only here: in the dependency cache it would rebuild it per commit.
        ARTISAN_COMMIT = commit;
        doNotPostBuildInstallCargoBinaries = true;
        installPhaseCommand = ''
          mkdir -p "$out/bin"
          install -m755 "${profileDirectory}/${host.exe name}" "$out/bin/${host.exe name}"
        '';
        meta.mainProgram = name;
      }
    );

  ae = binary {
    platform = "linux";
    package = "ae";
    name = "ae";
  };
  forge = binary {
    platform = "linux";
    package = "forge";
    name = "forge";
  };
  editorUnwrapped = binary {
    platform = "linux";
    package = "editor";
    name = "artisan-editor";
  };
  # The Nix Editor finds its graphics libraries in the store, plus the host
  # GPU drivers (NixOS, WSLg).
  editor =
    pkgs.runCommand "artisan-editor-${version}"
      {
        nativeBuildInputs = [ pkgs.makeWrapper ];
        passthru.unwrapped = editorUnwrapped;
        meta.mainProgram = "artisan-editor";
      }
      ''
        makeWrapper ${editorUnwrapped}/bin/artisan-editor "$out/bin/artisan-editor" \
          --prefix LD_LIBRARY_PATH : ${graphicsLibraryPath} \
          --run 'for dir in /run/opengl-driver/lib /usr/lib/wsl/lib; do [ -d "$dir" ] && LD_LIBRARY_PATH="$LD_LIBRARY_PATH:$dir"; done; export LD_LIBRARY_PATH'
      '';
  windows = lib.optionalAttrs pkgs.stdenv.hostPlatform.isx86_64 {
    ae-windows = binary {
      platform = "windows";
      package = "ae";
      name = "ae";
    };
    forge-windows = binary {
      platform = "windows";
      package = "forge";
      name = "forge";
    };
    artisan-editor-windows = binary {
      platform = "windows";
      package = "editor";
      name = "artisan-editor";
    };
  };
  release = lib.optionalAttrs pkgs.stdenv.hostPlatform.isx86_64 {
    release = import ./release.nix {
      inherit pkgs version;
      linux = {
        inherit ae forge;
        editor = editorUnwrapped;
      };
      windows = {
        ae = windows.ae-windows;
        forge = windows.forge-windows;
        editor = windows.artisan-editor-windows;
      };
    };
  };

  # Checks: clippy and the tests over the workspace, in the dev profile, on
  # one shared dependency cache.
  checkBase = platforms.linux.environment // {
    inherit src version;
    pname = "artisan-workspace";
    strictDeps = true;
    CARGO_PROFILE = "dev";
    CARGO_PROFILE_DEV_DEBUG = "0";
    CARGO_PROFILE_TEST_DEBUG = "0";
    cargoExtraArgs = "--locked --offline --workspace --all-targets";
  };
  checkDeps = craneLib.buildDepsOnly (
    checkBase
    // rerunnableBuild
    // {
      pname = "artisan-workspace-dependencies";
      # The build already covers every target; Crane's check adds a second
      # --all-targets and Cargo rejects the duplicated option.
      cargoCheckCommand = "true";
    }
  );
  checks = {
    # Cargo.toml owns lint policy.
    clippy = craneLib.cargoClippy (
      checkBase
      // {
        cargoArtifacts = checkDeps;
        cargoClippyExtraArgs = "";
      }
    );
    tests = craneLib.cargoTest (
      checkBase
      // {
        cargoArtifacts = checkDeps;
        # Snapshot mismatches fail instead of writing `.snap.new` files.
        INSTA_UPDATE = "no";
        CI = "true";
      }
    );
  };

  shell =
    extra:
    craneLib.devShell {
      packages = [
        pkgs.nixfmt
        pkgs.nix-direnv
        pkgs.cargo-nextest
        pkgs.python3
        pkgs.shellcheck
      ]
      ++ nativeTools
      ++ extra;
      buildInputs = libraries;
      inherit (platforms.linux.environment) LIBCLANG_PATH;
      shellHook = ''
        export LD_LIBRARY_PATH="${graphicsLibraryPath}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
        for dir in /usr/lib/wsl/lib /run/opengl-driver/lib; do
          if [ -d "$dir" ]; then export LD_LIBRARY_PATH="$LD_LIBRARY_PATH:$dir"; fi
        done
        export CARGO_BUILD_JOBS="''${CARGO_BUILD_JOBS:-2}"
      '';
    };
  # Shells for the dev loop's Cargo builds (`nix develop .#cargo-<platform>`):
  # each platform's toolchain, libraries, fast linker and cross environment
  # as the packages have them, outside the sandbox so a persistent target
  # directory compiles incrementally. The Windows shell builds for its target
  # by default.
  cargoShells = {
    linux = pkgs.mkShell (
      platforms.linux.fastLinker
      // {
        nativeBuildInputs = platforms.linux.fastLinker.nativeBuildInputs ++ [ rust ];
      }
    );
    windows = windowsPkgs.mkShell (
      platforms.windows.fastLinker
      // {
        nativeBuildInputs = platforms.windows.fastLinker.nativeBuildInputs ++ [ rust ];
      }
    );
  };
  dev = import ./dev.nix { inherit pkgs graphicsLibraryPath; };
  # `nix run .#release`: the dev loop with Cargo's release profile. The
  # signed release archives stay `nix build .#release` (packages, not apps).
  devRelease = import ./dev.nix {
    inherit pkgs graphicsLibraryPath;
    profile = "release";
  };
in
{
  inherit checks;
  formatter = pkgs.writeShellApplication {
    name = "artisan-nixfmt";
    runtimeInputs = [ pkgs.nixfmt ];
    text = ''
      if [ "$#" -eq 0 ]; then
        set -- flake.nix nix/*.nix
      fi
      exec nixfmt "$@"
    '';
  };
  devShells = {
    default = shell [ ];
    cargo-linux = cargoShells.linux;
    cargo-windows = cargoShells.windows;
    maintenance = shell [ pkgs.gh ];
    performance = shell [
      pkgs.perf
      pkgs.hyperfine
    ];
    visual = shell [
      pkgs.vulkan-tools
      pkgs.mesa-demos
    ];
  };
  packages = {
    default = ae;
    inherit ae forge;
    artisan-editor = editor;
  }
  // windows
  // release;
  apps = {
    default = dev;
    inherit dev;
    release = devRelease;
  };
}
