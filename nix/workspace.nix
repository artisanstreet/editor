{
  self,
  pkgs,
  windowsPkgs,
  crane,
}:
let
  inherit (pkgs) lib;
  version = (builtins.fromTOML (builtins.readFile ../Cargo.toml)).workspace.package.version;
  rust = pkgs.rust-bin.fromRustupToolchainFile ../rust-toolchain.toml;
  craneLib = (crane.mkLib pkgs).overrideToolchain rust;
  # Keep embedded fonts, licenses, schemas, JSON and external tests. Never copy
  # local build products, captures, databases or the private handoff ledger.
  src = lib.cleanSourceWith {
    src = ../.;
    filter =
      path: type:
      let
        relative = lib.removePrefix (toString ../. + "/") (toString path);
        parts = lib.splitString "/" relative;
        root = builtins.head parts;
      in
      (builtins.elem root [
        "Cargo.toml"
        "Cargo.lock"
        "rust-toolchain.toml"
        "rustfmt.toml"
        ".cargo"
        ".config"
        "modules"
        "tests"
        "scripts"
        "packaging"
      ])
      && !(builtins.any (
        part:
        builtins.elem part [
          "target"
          "__pycache__"
          ".dist"
          ".git"
        ]
      ) parts)
      && !(lib.hasSuffix ".pyc" relative);
  };
  libraries = with pkgs; [
    openssl
    sqlite
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
  ];
  nativeTools = with pkgs; [
    git
    pkg-config
    clang
    cmake
    ninja
    python3
    capnproto
  ];
  common = {
    inherit src;
    pname = "artisan-workspace";
    inherit version;
    strictDeps = true;
    nativeBuildInputs = nativeTools;
    buildInputs = libraries;
    LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
    CARGO_PROFILE_DEV_DEBUG = "0";
    CARGO_PROFILE_TEST_DEBUG = "0";
  };
  vendor = craneLib.vendorCargoDeps { inherit src; };
  base = common // {
    cargoVendorDir = vendor;
  };
  publicTrust = builtins.listToAttrs (
    map
      (
        line:
        let
          pieces = lib.splitString "=" line;
        in
        {
          name = builtins.head pieces;
          value = lib.concatStringsSep "=" (builtins.tail pieces);
        }
      )
      (
        lib.filter (line: line != "" && !(lib.hasPrefix "#" line)) (
          lib.splitString "\n" (builtins.readFile ../modules/installer/release/trust_anchor.env)
        )
      )
  );
  releaseTrust = {
    ARTISAN_INSTALLER_RELEASE = "1";
    inherit (publicTrust) ARTISAN_RELEASE_KEY_ID ARTISAN_RELEASE_PUBLIC_KEY_HEX;
  };

  # The product: Debug and Production payloads per platform (nix/stages.nix).
  stageBuilds = import ./stages.nix {
    inherit
      self
      lib
      pkgs
      windowsPkgs
      crane
      src
      version
      libraries
      nativeTools
      releaseTrust
      ;
  };
  production = stageBuilds.packages.linux-production;

  # Tests, fixtures, and tools build in Cargo's dev profile against one shared
  # dependency cache; they are never shipped.
  allTargets = "--locked --offline --workspace --all-targets --features artisan-frontend/visual-proof";
  devDeps = craneLib.buildDepsOnly (
    base
    // {
      pname = "artisan-dependencies-dev";
      CARGO_PROFILE = "dev";
      cargoExtraArgs = allTargets;
      doCheck = false;
    }
  );
  # Each tool compiles only its own dependency graph, so a small tool never
  # waits on the workspace-wide cache.
  tool =
    package: name:
    let
      attrs = base // {
        pname = "artisan-${name}";
        CARGO_PROFILE = "dev";
        cargoExtraArgs = "--locked --offline -p ${package} --bin ${name}";
        doCheck = false;
      };
    in
    craneLib.buildPackage (
      attrs
      // {
        cargoArtifacts = craneLib.buildDepsOnly (attrs // { pname = "artisan-${name}-dependencies"; });
        meta.mainProgram = name;
      }
    );
  generator = tool "artisan-packaging" "payload-manifest-generator";
  releaseTool = tool "artisan-packaging" "release-tool";
  plugin = tool "artisan-capnp-codegen" "artisan-capnp-codegen";
  hostLauncher = tool "artisan-native-dev" "forge-host";
  screenDemo = tool "artisan-screen-demo" "screen-demo";
  # Remote-host fixtures run the product binaries; test builds do not need
  # the Production codegen.
  testProduct = craneLib.buildPackage (
    base
    // {
      pname = "artisan-test-product";
      CARGO_PROFILE = "dev";
      cargoArtifacts = devDeps;
      cargoExtraArgs = "--locked --offline ${
        lib.concatMapStringsSep " " (
          binary: "-p ${binary.package} --bin ${binary.name}"
        ) stageBuilds.binaries
      }";
      doCheck = false;
    }
  );

  # Cap'n Proto bindings: the Nix-pinned compiler with the Cargo-pinned plugin.
  generateBindings = output: ''
    mkdir -p "${output}"
    capnp compile --no-standard-import --src-prefix=schema \
      -o${plugin}/bin/artisan-capnp-codegen:"${output}" \
      schema/phase1_proof.capnp schema/artisan.capnp schema/composer_state.capnp
  '';
  bindings =
    pkgs.runCommand "artisan-generated-bindings" { nativeBuildInputs = [ pkgs.capnproto ]; }
      ''
        cd ${src}/modules/protocol
        ${generateBindings "$out"}
      '';

  # Portable payload archive of a payload's binaries: stored ZIP plus
  # payload-manifest.json, byte-reproducible (payload-manifest-generator).
  payloadArchive =
    payload:
    pkgs.runCommand "artisan-payload.zip" { } ''
      ${generator}/bin/payload-manifest-generator \
        --layout ${../packaging/portable/versioned_layout.txt} --archive "$out" \
        ${lib.concatMapStringsSep " " (
          binary: "--file bin/${binary.name} ${payload}/bin/${binary.name}"
        ) stageBuilds.binaries}
    '';
  archive = payloadArchive production;
  architecture = if pkgs.stdenv.hostPlatform.isAarch64 then "arm64" else "x64";
  releaseMetadata = (builtins.fromJSON (builtins.readFile ../packaging/release/development.json)) // {
    "product-version" = version;
    "editor-forge-compatibility-version" = version;
    "platform" = "linux";
    "architecture" = architecture;
    "libc" = "glibc";
    "artifact-id" = "linux-${architecture}-nix";
    "file-name" = "artisan-nix-payload.zip";
    "signing-key-id" = releaseTrust.ARTISAN_RELEASE_KEY_ID;
  };
  unsignedManifest = pkgs.runCommand "artisan-unsigned-release.json" { } ''
    ${releaseTool}/bin/release-tool generate --archive ${archive} --output "$out" \
      ${lib.cli.toGNUCommandLineShell { } releaseMetadata}
  '';

  graphicalEnvironment = ''
    export LD_LIBRARY_PATH="${
      lib.makeLibraryPath (libraries ++ [ pkgs.mesa ])
    }''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
    if [ -d /usr/lib/wsl/lib ]; then
      export LD_LIBRARY_PATH="$LD_LIBRARY_PATH:/usr/lib/wsl/lib"
    fi
    if [ -d /run/opengl-driver/lib ]; then
      export LD_LIBRARY_PATH="$LD_LIBRARY_PATH:/run/opengl-driver/lib"
    fi
  '';
  shellApp =
    name: runtimeInputs: text:
    pkgs.writeShellApplication { inherit name runtimeInputs text; };
  app = derivation: name: {
    type = "app";
    program = "${derivation}/bin/${name}";
  };
  closure = pkgs.closureInfo { rootPaths = [ production ]; };
  devLoop = import ./dev.nix {
    inherit lib pkgs graphicalEnvironment;
    linuxRunner = stageBuilds.runner "linux";
  };
  parity = craneLib.buildPackage (
    base
    // {
      pname = "artisan-parity-visual-proof";
      CARGO_PROFILE = "dev";
      cargoArtifacts = devDeps;
      cargoExtraArgs = "--locked --offline -p artisan-frontend --example parity-visual-proof --features visual-proof";
      doCheck = false;
      installPhaseCommand = ''
        mkdir -p "$out/bin"
        cp target/debug/examples/parity-visual-proof "$out/bin/"
      '';
    }
  );
  visualApp = shellApp "artisan-visual-proof" [ pkgs.coreutils ] (
    graphicalEnvironment
    + ''
      output="''${ARTISAN_VISUAL_OUTPUT:-$PWD/evidence/visual}"
      mkdir -p "$output"
      cd "$output"
      exec timeout --signal=TERM --kill-after=5s 45s ${parity}/bin/parity-visual-proof "$@"
    ''
  );
  # Rewrites the checked-in bindings in the working tree.
  codegenApp = shellApp "artisan-codegen" [ pkgs.capnproto pkgs.git ] ''
    cd "$(git rev-parse --show-toplevel)/modules/protocol"
    ${generateBindings "src"}
  '';
  pinApp = shellApp "artisan-verify-gpui-pin" [ pkgs.python3 pkgs.gh ] ''
    exec python "$PWD/scripts/verify_gpui_pin.py" "$@"
  '';
  checkBase = base // {
    CARGO_PROFILE = "dev";
    cargoArtifacts = devDeps;
    cargoExtraArgs = allTargets;
  };
  simpleCheck =
    name: inputs: command:
    pkgs.runCommand name { nativeBuildInputs = inputs; } ''
      cd ${src}
      ${command}
      touch "$out"
    '';
  # Test fixtures for a Cargo test run over target/debug: the example
  # fixture programs, and two independently produced payload archives of
  # small stand-in binaries (debug UI binaries exceed a gigabyte and obscure
  # the archive structure and tampering proofs).
  testFixtures = ''
    export ARTISAN_ENGINE_OWNER_FIXTURE="$PWD/target/debug/examples/engine-owner-fixture"
    export ARTISAN_CODEX_WIRE_FIXTURE="$PWD/target/debug/examples/codex-wire-fixture"
    export ARTISAN_DIRECTORY_CONTROLLER_FIXTURE="$PWD/target/debug/examples/directory-controller-fixture"
    fixtures="$(mktemp -d)"
    files=()
    for name in ae editor forge installer; do
      printf 'archive fixture: %s' "$name" > "$fixtures/$name"
      files+=(--file "bin/$name" "$fixtures/$name")
      export "ARTISAN_VERSIONED_PAYLOAD_''${name^^}_BINARY=$fixtures/$name"
    done
    for archive in payload repeat; do
      target/debug/payload-manifest-generator --layout packaging/portable/versioned_layout.txt \
        --archive "$fixtures/$archive.zip" "''${files[@]}"
    done
    export ARTISAN_VERSIONED_PAYLOAD_ARCHIVE="$fixtures/payload.zip"
    export ARTISAN_VERSIONED_PAYLOAD_ARCHIVE_REPRODUCIBILITY="$fixtures/repeat.zip"
  '';
  nextest =
    scope: features:
    "cargo nextest run --locked --offline --no-fail-fast ${scope} --all-targets ${features} --test-threads=1";
  shell =
    extra:
    craneLib.devShell {
      checks = { inherit (checks) clippy; };
      packages = [
        rust
        pkgs.git
        pkgs.python3
        pkgs.capnproto
        pkgs.nixfmt
        pkgs.nix-direnv
        pkgs.cargo-nextest
      ]
      ++ nativeTools
      ++ extra;
      buildInputs = libraries;
      LIBCLANG_PATH = common.LIBCLANG_PATH;
      shellHook = graphicalEnvironment + ''
        export CARGO_BUILD_JOBS="''${CARGO_BUILD_JOBS:-2}"
      '';
    };
  checks = {
    rustfmt = craneLib.cargoFmt {
      inherit src;
      pname = "artisan-rustfmt";
      inherit version;
    };
    # Cargo.toml owns lint policy.
    clippy = craneLib.cargoClippy (checkBase // { cargoClippyExtraArgs = ""; });
    registration = craneLib.mkCargoDerivation (
      checkBase
      // {
        pname = "artisan-test-registration";
        buildPhaseCargoCommand = "python scripts/audit_rust_target_registration.py";
        doInstallCargoArtifacts = false;
        installPhaseCommand = ''touch "$out"'';
      }
    );
    file-sizes = simpleCheck "artisan-file-sizes" [
      pkgs.python3
    ] "python scripts/file_size_ratchet.py";
    remote-hosts = simpleCheck "artisan-remote-hosts" [ pkgs.python3 pkgs.git pkgs.getent ] ''
      python ${src}/scripts/test_remote_host.py --bin-dir ${testProduct}/bin --helper ${hostLauncher}/bin/forge-host
    '';
    python = simpleCheck "artisan-python-tests" [
      pkgs.python3
    ] "python -B -m unittest discover -s scripts/tests";
    codegen = pkgs.runCommand "artisan-codegen-drift" { } ''
      diff -u ${src}/modules/protocol/src/artisan_capnp.rs ${bindings}/artisan_capnp.rs
      diff -u ${src}/modules/protocol/src/phase1_proof_capnp.rs ${bindings}/phase1_proof_capnp.rs
      diff -u ${src}/modules/protocol/src/composer_state_capnp.rs ${bindings}/composer_state_capnp.rs
      touch "$out"
    '';
    tests = craneLib.mkCargoDerivation (
      checkBase
      // {
        pname = "artisan-tests";
        NEXTEST_SHOW_PROGRESS = "counter";
        nativeBuildInputs = nativeTools ++ [ pkgs.cargo-nextest ];
        buildPhaseCargoCommand = ''
          cargo build --locked --offline --workspace --bins --examples --features artisan-frontend/visual-proof
          ${testFixtures}
          ${nextest "--workspace" "--features artisan-frontend/visual-proof"}
        '';
        doInstallCargoArtifacts = false;
        installPhaseCommand = ''touch "$out"'';
      }
    );
    packaging = craneLib.mkCargoDerivation (
      checkBase
      // {
        pname = "artisan-packaging-tests";
        nativeBuildInputs = nativeTools ++ [ pkgs.cargo-nextest ];
        NEXTEST_SHOW_PROGRESS = "counter";
        buildPhaseCargoCommand = ''
          cargo build --locked --offline -p artisan-packaging --bins
          ${testFixtures}
          ${nextest "-p artisan-packaging" ""}
        '';
        doInstallCargoArtifacts = false;
        installPhaseCommand = ''touch "$out"'';
      }
    );
    visual-fixtures = craneLib.cargoTest (
      checkBase
      // {
        pname = "artisan-visual-fixtures";
        cargoExtraArgs = "--locked --offline -p artisan-frontend --lib --features visual-proof";
        cargoTestExtraArgs = "parity_visual_proof::tests -- --test-threads=1";
      }
    );
    doc-tests = craneLib.cargoDocTest (
      checkBase // { cargoExtraArgs = "--locked --offline --workspace"; }
    );
    workflows =
      pkgs.runCommand "artisan-workflow-validation" { nativeBuildInputs = [ pkgs.actionlint ]; }
        ''
          actionlint -config-file ${../.github/actionlint.yaml} ${../.github/workflows/ci.yml} ${../.github/workflows/desktop.yml}
          touch "$out"
        '';
    nixfmt = pkgs.runCommand "artisan-nix-format" { nativeBuildInputs = [ pkgs.nixfmt ]; } ''
      nixfmt --check ${../flake.nix} ${./workspace.nix} ${./stages.nix} ${./dev.nix} ${./forge-service.nix}
      touch "$out"
    '';
  };
in
{
  inherit checks;
  formatter = shellApp "artisan-nixfmt" [ pkgs.nixfmt ] ''
    if [ "$#" -eq 0 ]; then
      set -- flake.nix nix/*.nix
    fi
    exec nixfmt "$@"
  '';
  devShells = {
    default = shell [ ];
    maintenance = shell [ pkgs.gh ];
    performance = shell [
      pkgs.linuxPackages.perf
      pkgs.hyperfine
    ];
    visual = shell [
      pkgs.vulkan-tools
      pkgs.mesa-demos
    ];
  };
  packages = stageBuilds.packages // {
    default = production;
    dev-dependencies = devDeps;
    payload-manifest-generator = generator;
    release-tool = releaseTool;
    capnp-codegen = plugin;
    generated-bindings = bindings;
    forge-host = hostLauncher;
    screen-demo = screenDemo;
    parity-visual-proof = parity;
    nix-payload = archive;
    unsigned-release = unsignedManifest;
    # Registration data and path list for the complete Nix runtime closure.
    inherit closure;
    forge-container = pkgs.dockerTools.buildLayeredImage {
      name = "artisan-forge";
      tag = version;
      contents = [
        pkgs.git
        pkgs.getent
        pkgs.coreutils
        pkgs.cacert
      ];
      config = {
        Entrypoint = [ "${production}/bin/forge" ];
        WorkingDir = "/data";
        User = "65532:65532";
        Env = [
          "SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
          "HOME=/data"
          "PATH=${
            lib.makeBinPath [
              pkgs.git
              pkgs.getent
              pkgs.coreutils
            ]
          }"
        ];
      };
    };
  };
  apps = {
    default = app devLoop "artisan-dev";
    dev = app devLoop "artisan-dev";
    codegen = app codegenApp "artisan-codegen";
    verify-gpui-pin = app pinApp "artisan-verify-gpui-pin";
    visual-proof = app visualApp "artisan-visual-proof";
    visual-proof-software = app (shellApp "artisan-visual-proof-software" [ ] ''
      export VK_DRIVER_FILES="${pkgs.mesa}/share/vulkan/icd.d/lvp_icd.${pkgs.stdenv.hostPlatform.parsed.cpu.name}.json"
      exec ${visualApp}/bin/artisan-visual-proof "$@"
    '') "artisan-visual-proof-software";
    capture-screen-demo = app (shellApp "artisan-capture-screen-demo"
      [ pkgs.python3 pkgs.xdotool pkgs.imagemagick ]
      (
        graphicalEnvironment
        + ''
          exec python ${src}/scripts/capture_window.py \
            --program ${screenDemo}/bin/screen-demo \
            --software-icd ${pkgs.mesa}/share/vulkan/icd.d/lvp_icd.${pkgs.stdenv.hostPlatform.parsed.cpu.name}.json "$@"
        ''
      )
    ) "artisan-capture-screen-demo";
    screen-demo = app (shellApp "artisan-screen-demo" [ ] (
      graphicalEnvironment
      + ''
        exec ${screenDemo}/bin/screen-demo "$@"
      ''
    )) "artisan-screen-demo";
    release-tool = app releaseTool "release-tool";
    export-closure = app (shellApp "artisan-export-closure" [ pkgs.nix ] ''
      if [ "$#" -ne 1 ]; then
        echo 'usage: nix run .#export-closure -- /absolute/output.nar' >&2
        exit 2
      fi
      mapfile -t paths < ${closure}/store-paths
      nix-store --export "''${paths[@]}" > "$1"
    '') "artisan-export-closure";
    sign-release = app (shellApp "artisan-sign-release" [ ] ''
      exec ${releaseTool}/bin/release-tool sign "$@"
    '') "artisan-sign-release";
  };
}
