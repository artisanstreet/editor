# Build stages: the product payload for each platform, in two stages
# (Cargo.toml `production-debug` / `production`): Debug built for speed,
# Production for runtime performance.
#
# A payload is the four product binaries plus the build identity the
# installed binaries report (resources/build-info.json). Nix owns everything
# around the compiler: toolchain, cross toolchain, dependency caching,
# identity, and layout. Cargo only compiles Rust.
{
  self,
  lib,
  pkgs,
  windowsPkgs,
  crane,
  src,
  version,
  libraries,
  nativeTools,
  releaseTrust,
  graphicsLibraryPath,
  rerunnableBuild,
}:
let
  # Debug favours build speed; Production favours runtime speed (Cargo.toml
  # has the codegen of each). `payload` holds each stage's own derivation
  # attributes.
  stages = {
    debug = {
      profile = "production-debug";
      features = [ "artisan-frontend/debug-tools" ];
      # mold / lld instead of GNU ld (`fastLinker` per platform).
      fastLink = true;
      payload = {
        # No CARGO_BUILD_JOBS limit: without whole-program LTO the links are
        # fast and small, so Cargo uses every core (crane's default,
        # NIX_BUILD_CORES). The stdenv fixup would strip the debug info this
        # stage exists to keep.
        dontStrip = true;
        # Rewriting every toolchain and vendored-source path out of a GB of
        # debug info takes about a minute per payload, and those paths are
        # what lets a debugger find the sources. The Debug payload is only
        # ever installed on the machine that built it.
        doNotRemoveReferencesToVendorDir = true;
        doNotRemoveReferencesToRustToolchain = true;
      };
    };
    production = {
      profile = "production";
      features = [ ];
      fastLink = false;
      payload = {
        # Each binary ends in a single-threaded fat-LTO link; the Editor's
        # peaks near 5.5 GB. At one job per core four links overlap and
        # exhaust a 16 GB machine. The dependencies are already compiled,
        # so linking one at a time mostly serializes work that was
        # single-threaded anyway.
        CARGO_BUILD_JOBS = 1;
      };
    };
  };
  binaries = [
    {
      package = "artisan-frontend";
      name = "editor";
    }
    {
      package = "artisan-backend";
      name = "forge";
    }
    {
      package = "artisan-editor-cli";
      name = "ae";
    }
    {
      package = "ae-installer";
      name = "installer";
    }
  ];
  selection = lib.concatMapStringsSep " " (
    binary: "-p ${binary.package} --bin ${binary.name}"
  ) binaries;
  featureArgs =
    features: lib.optionalString (features != [ ]) "--features ${lib.concatStringsSep "," features}";

  # Flake identity: the commit, and whether the tree had uncommitted changes.
  commit = self.rev or self.dirtyRev or null;
  dirty = !(self ? rev);
  shortCommit =
    if commit == null then null else builtins.substring 0 10 (lib.removeSuffix "-dirty" commit);
  revCount = toString (self.revCount or 0);

  # Every stage build carries an identity; the output hash in the version
  # makes each distinct payload its own installable version.
  identityScript =
    {
      stage,
      channel,
      target,
    }:
    ''
      mkdir -p "$out/resources"
      output_hash="$(basename "$out" | cut -c1-10)"
      metadata="${
        lib.concatStringsSep "." (
          lib.filter (part: part != null) [
            (if shortCommit == null then null else "g${shortCommit}")
            (if dirty then "dirty" else null)
          ]
        )
      }"
      version="${version}-${channel}.${revCount}+''${metadata:+$metadata.}n$output_hash"
      cat > "$out/resources/build-info.json" <<EOF
      {
        "format_version": 1,
        "version": "$version",
        "channel": "${channel}",
        "commit": ${if commit == null then "null" else ''"${lib.removeSuffix "-dirty" commit}"''},
        "dirty": ${lib.boolToString dirty},
        "profile": "${stages.${stage}.profile}",
        "target": "${target}",
        "built_at": null
      }
      EOF
    '';

  platforms = {
    linux = rec {
      target = pkgs.stdenv.hostPlatform.rust.rustcTarget;
      exe = name: name;
      craneLib = (crane.mkLib pkgs).overrideToolchain (
        p: p.rust-bin.fromRustupToolchainFile ../rust-toolchain.toml
      );
      build = attrs: craneLib.buildPackage attrs;
      deps = attrs: craneLib.buildDepsOnly attrs;
      environment = {
        nativeBuildInputs = nativeTools;
        buildInputs = libraries;
        LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
      };
      # mold, wrapped by nixpkgs like the default linker (so store rpaths and
      # the dynamic linker still apply), links in seconds what GNU ld takes
      # minutes over.
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
      # LLVM's lld in its MinGW mode, wrapped by nixpkgs for this target so
      # it sees the same library paths as GNU ld. GCC finds it as its `ld` in
      # a directory of its own (`-B`); only the linker is exposed, since the
      # rest of the LLVM binutils (llvm-windres above all) would shadow the
      # GNU tools the build scripts expect. MinGW GCC 15 asks the linker for
      # `-pie`, which only GNU ld understands; lld's PE output is
      # relocatable anyway (Rust passes `--dynamicbase` itself).
      lld = pkgs.runCommand "x86_64-w64-mingw32-lld" { } ''
        mkdir -p "$out/bin"
        ln -s ${windowsPkgs.buildPackages.llvmPackages.bintools}/bin/${windowsPkgs.stdenv.cc.targetPrefix}ld.lld "$out/bin/ld"
      '';
      fastLinker = environment // {
        CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS = lib.concatStringsSep " " (
          rustflags
          ++ [
            "-C link-arg=-B${lld}/bin/"
            "-C link-arg=-no-pie"
          ]
        );
      };
      # nixpkgs builds MinGW GCC with the mcf thread model: C code using
      # thread-locals (mimalloc) pulls in the GCC runtime's emutls, which
      # needs mcfgthread. Link its static archive (a plain -lmcfgthread
      # picks the DLL import library) and the NT libraries it calls, so
      # every .exe imports only Windows system DLLs. This replaces the
      # target's rustflags from .cargo/config.toml, so it repeats them.
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
          nativeBuildInputs = nativeTools;
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
          HOST_CC = "${pkgs.stdenv.cc}/bin/cc";
          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
        };
    };
  };

  payload =
    {
      platform,
      stage,
      channel ? "dev",
    }:
    let
      host = platforms.${platform};
      settings = stages.${stage};
      common =
        (if settings.fastLink then host.fastLinker else host.environment)
        // lib.optionalAttrs (stage == "production") releaseTrust
        // rerunnableBuild
        // {
          inherit src version;
          strictDeps = true;
          CARGO_PROFILE = settings.profile;
          cargoExtraArgs = "--locked --offline ${selection} ${featureArgs settings.features}";
          doCheck = false;
        };
      dependencies = host.deps (common // { pname = "artisan-${platform}-${stage}-dependencies"; });
      profileDirectory = "target/${
        lib.optionalString (platform == "windows") "${host.target}/"
      }${settings.profile}";
    in
    host.build (
      common
      // {
        pname = "artisan-${platform}-${stage}";
        cargoArtifacts = dependencies;
        # One Cargo invocation builds all four binaries, so shared crates
        # compile once; the payload holds exactly those four and the identity.
        # Moving rather than copying them saves copying a GB of Debug
        # binaries.
        installPhaseCommand = ''
          mkdir -p "$out/bin"
          for binary in ${lib.concatMapStringsSep " " (binary: host.exe binary.name) binaries}; do
            mv "${profileDirectory}/$binary" "$out/bin/$binary"
          done
          ${identityScript {
            inherit stage channel;
            inherit (host) target;
          }}
        '';
        passthru = { inherit platform stage; };
      }
      // settings.payload
    );
  # The dev runner installs payloads; a plain release build is enough for a
  # tool. Its installer library compiles as a release build, so it carries
  # the public trust anchor like any release installer. The Linux runner
  # records where the Linux Editor loads its graphics libraries from.
  runner =
    platform:
    let
      host = platforms.${platform};
      common =
        host.environment
        // releaseTrust
        // rerunnableBuild
        // lib.optionalAttrs (platform == "linux") {
          ARTISAN_DEV_GRAPHICS_LIBRARY_PATH = graphicsLibraryPath;
        }
        // {
          inherit src version;
          strictDeps = true;
          CARGO_PROFILE = "release";
          cargoExtraArgs = "--locked --offline -p artisan-native-dev --bin dev";
          doCheck = false;
        };
      profileDirectory = "target/${lib.optionalString (platform == "windows") "${host.target}/"}release";
    in
    host.build (
      common
      // {
        pname = "artisan-${platform}-runner";
        cargoArtifacts = host.deps (common // { pname = "artisan-${platform}-runner-dependencies"; });
        installPhaseCommand = ''
          mkdir -p "$out/bin"
          install -m755 "${profileDirectory}/${host.exe "dev"}" "$out/bin/${host.exe "dev"}"
        '';
        meta.mainProgram = "dev";
      }
    );
in
{
  inherit
    stages
    binaries
    payload
    runner
    ;
  packages = {
    linux-runner = runner "linux";
    windows-runner = runner "windows";
  }
  // lib.listToAttrs (
    lib.concatMap (
      platform:
      map (stage: {
        name = "${platform}-${stage}";
        value = payload { inherit platform stage; };
      }) (builtins.attrNames stages)
    ) (builtins.attrNames platforms)
  );
}
