{
  self,
  pkgs,
  windowsPkgs,
  crane,
  blaze,
}:
let
  inherit (pkgs) lib;
  manifest = builtins.fromTOML (builtins.readFile ../Cargo.toml);
  inherit (manifest.workspace.package) version;
  rust = pkgs.rust-bin.fromRustupToolchainFile ../rust-toolchain.toml;
  craneLib = (crane.mkLib pkgs).overrideToolchain rust;
  shaderTools = import ./shaders.nix { inherit pkgs; };

  # The crates plus what they embed (skills/) and Cargo's configuration.
  roots = [
    "crates"
    "vendor/highlighting"
    "vendor/mermaid-rs-renderer"
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

  libraries = with pkgs; [
    fontconfig
    freetype
    wayland
    libX11
    libxcb
    openssl
    sqlite
  ];
  libraryPath = lib.makeLibraryPath libraries;
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
          nativeBuildInputs = nativeTools ++ [
            pkgs.vkd3d
            pkgs.directx-shader-compiler
            shaderTools.fxc
          ];
          AQUA_DXC = "${pkgs.directx-shader-compiler}/bin/dxc";
          AQUA_FXC = "${shaderTools.fxc}/bin/aqua-fxc";
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

  binary =
    {
      platform,
      package,
      name,
      bin ? name,
    }:
    let
      host = platforms.${platform};
      common =
        host.environment
        // rerunnableBuild
        // {
          inherit src version;
          pname = "${name}-${platform}";
          preBuild =
            writableTarget
            + lib.optionalString (platform == "windows" && package == "editor-aqua") shaderTools.prepare;
          postBuild = lib.optionalString (
            platform == "windows" && package == "editor-aqua"
          ) shaderTools.cleanup;
          strictDeps = true;
          CARGO_PROFILE = "release";
          cargoExtraArgs = "--locked --offline -p ${package} --bin ${bin}";
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
        # Only here: in the dependency cache they would rebuild it per commit.
        ARTISAN_COMMIT = commit;
        EDITOR_COMMIT = commit;
        doNotPostBuildInstallCargoBinaries = true;
        installPhaseCommand = ''
          mkdir -p "$out/bin"
          install -m755 "${profileDirectory}/${host.exe bin}" "$out/bin/${host.exe name}"
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
      package = "editor-aqua";
      name = "artisan-editor";
      bin = "editor-aqua";
    };
  };
  release = lib.optionalAttrs pkgs.stdenv.hostPlatform.isx86_64 {
    release = import ./release.nix {
      inherit pkgs version;
      linux = {
        inherit ae forge;
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
    # Cargo.toml owns lint policy. The commit rides only the final
    # derivation: the shared dependency cache below stays commit-free.
    clippy = craneLib.cargoClippy (
      checkBase
      // {
        cargoArtifacts = checkDeps;
        cargoClippyExtraArgs = "";
        EDITOR_COMMIT = commit;
      }
    );
    tests = craneLib.cargoTest (
      checkBase
      // {
        cargoArtifacts = checkDeps;
        # Snapshot mismatches fail instead of writing `.snap.new` files.
        INSTA_UPDATE = "no";
        CI = "true";
        EDITOR_COMMIT = commit;
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
        export LD_LIBRARY_PATH="${libraryPath}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
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
  dev = import ./dev.nix { inherit pkgs; };
  # Upstream release packaging rejects Nix glibc; build its pinned source here.
  devfast = import ./dev.nix {
    inherit pkgs;
    blazePackage =
      let
        upstream = import blaze.inputs.nixpkgs {
          system = pkgs.stdenv.hostPlatform.system;
          overlays = [ blaze.inputs.rust-overlay.overlays.default ];
        };
        pin = upstream.lib.removePrefix "nightly-" (
          upstream.lib.trim (builtins.readFile "${blaze.outPath}/harness/toolchain")
        );
        toolchain = upstream.rust-bin.nightly.${pin}.default.override { extensions = [ "rust-src" ]; };
        platform = upstream.makeRustPlatform {
          cargo = toolchain;
          rustc = toolchain;
        };
      in
      platform.buildRustPackage {
        pname = "artisan-blaze";
        version = "0.1.0";
        src = blaze.outPath;
        cargoLock.lockFile = "${blaze.outPath}/Cargo.lock";
        cargoBuildFlags = [
          "-p"
          "ra-jit"
        ];
        doCheck = false;
        nativeBuildInputs = [ upstream.makeWrapper ];
        postFixup = ''
          for bin in "$out"/bin/*; do
            wrapProgram "$bin" --prefix PATH : ${toolchain}/bin
          done
        '';
      };
  };
  # `nix run .#release`: the dev loop with Cargo's release profile. The
  # signed release archives stay `nix build .#release` (packages, not apps).
  devRelease = import ./dev.nix {
    inherit pkgs;
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
  };
  packages = {
    default = ae;
    inherit ae forge;
  }
  // windows
  // release;
  apps = {
    default = dev;
    inherit dev devfast;
    release = devRelease;
  };
}
