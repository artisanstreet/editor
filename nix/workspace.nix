{ pkgs, crane }:
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
  allTargets = "--locked --offline --workspace --all-targets --features artisan-frontend/visual-proof";
  deps =
    profile:
    craneLib.buildDepsOnly (
      base
      // lib.optionalAttrs (profile == "release") releaseTrust
      // lib.optionalAttrs (profile == "performance") {
        CARGO_PROFILE_PERFORMANCE_DEBUG = "1";
        dontStrip = true;
      }
      // {
        pname = "artisan-dependencies-${profile}";
        CARGO_PROFILE = profile;
        cargoExtraArgs = allTargets;
        doCheck = false;
      }
    );
  devDeps = deps "dev";
  releaseDeps = deps "release";
  performanceDeps = deps "performance";
  # One compatible dependency cache per profile; helper outputs only build their
  # selected package and can be requested independently of product binaries.
  binary =
    profile: package: name:
    craneLib.buildPackage (
      base
      // lib.optionalAttrs (profile == "release") releaseTrust
      // lib.optionalAttrs (profile == "performance") {
        CARGO_PROFILE_PERFORMANCE_DEBUG = "1";
        dontStrip = true;
      }
      // {
        pname = "artisan-${name}-${profile}";
        CARGO_PROFILE = profile;
        cargoArtifacts =
          if profile == "release" then
            releaseDeps
          else if profile == "performance" then
            performanceDeps
          else
            devDeps;
        cargoExtraArgs = "--locked --offline -p ${package} --bin ${name}";
        doCheck = false;
        meta.mainProgram = name;
      }
    );
  product =
    profile:
    pkgs.symlinkJoin {
      name = "artisan-editor-${profile}";
      paths = [
        (binary profile "artisan-frontend" "editor")
        (binary profile "artisan-backend" "forge")
        (binary profile "artisan-editor-cli" "ae")
        (binary profile "ae-installer" "installer")
      ];
    };
  helper =
    package: name:
    craneLib.buildPackage (
      base
      // {
        pname = "artisan-${name}";
        CARGO_PROFILE = "dev";
        cargoArtifacts = craneLib.buildDepsOnly (
          base
          // {
            pname = "artisan-${name}-dependencies";
            CARGO_PROFILE = "dev";
            cargoExtraArgs = "--locked --offline -p ${package} --bin ${name}";
            doCheck = false;
          }
        );
        cargoExtraArgs = "--locked --offline -p ${package} --bin ${name}";
        doCheck = false;
        meta.mainProgram = name;
      }
    );
  generator = helper "artisan-packaging" "payload-manifest-generator";
  releaseTool = helper "artisan-packaging" "release-tool";
  plugin = helper "artisan-capnp-codegen" "artisan-capnp-codegen";
  launcher = helper "artisan-native-dev" "dev";
  hostLauncher = helper "artisan-native-dev" "forge-host";
  release = product "release";
  development = product "dev";
  performance = product "performance";
  bindings =
    pkgs.runCommand "artisan-generated-bindings"
      {
        nativeBuildInputs = [
          pkgs.python3
          pkgs.capnproto
        ];
      }
      ''
        python ${src}/scripts/codegen.py --plugin ${plugin}/bin/artisan-capnp-codegen --output "$out"
      '';
  archive = pkgs.runCommand "artisan-nix-payload.zip" { nativeBuildInputs = [ pkgs.python3 ]; } ''
    python ${src}/scripts/package.py --bin-dir ${release}/bin \
      --generator ${generator}/bin/payload-manifest-generator --output "$out"
  '';
  architecture = if pkgs.stdenv.hostPlatform.isAarch64 then "arm64" else "x64";
  metadata = pkgs.writeText "release-metadata.json" (
    builtins.toJSON (
      (builtins.fromJSON (builtins.readFile ../packaging/release/development.json))
      // {
        "product-version" = version;
        "editor-forge-compatibility-version" = version;
        "platform" = "linux";
        "architecture" = architecture;
        "libc" = "glibc";
        "artifact-id" = "linux-${architecture}-nix";
        "file-name" = "artisan-nix-payload.zip";
        "signing-key-id" = releaseTrust.ARTISAN_RELEASE_KEY_ID;
      }
    )
  );
  unsignedManifest =
    pkgs.runCommand "artisan-unsigned-release.json" { nativeBuildInputs = [ pkgs.python3 ]; }
      ''
        python ${src}/scripts/release_manifest.py --tool ${releaseTool}/bin/release-tool \
          --archive ${archive} --metadata ${metadata} --output "$out"
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
  launch =
    profile: binaries:
    shellApp "artisan-${profile}" [ pkgs.git pkgs.getent ] (
      graphicalEnvironment
      + ''
        state="''${ARTISAN_DEV_DIR:-''${XDG_STATE_HOME:-$HOME/.local/state}/artisan/${profile}}"
        exec ${launcher}/bin/dev --bin-dir ${binaries}/bin --dev-dir "$state" "$@"
      ''
    );
  closure = pkgs.closureInfo {
    rootPaths = [
      release
      editorApp
    ];
  };
  devApp = launch "dev" development;
  editorApp = launch "release" release;
  performanceApp = launch "performance" performance;
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
  codegenApp = shellApp "artisan-codegen" [ pkgs.python3 pkgs.capnproto ] ''
    exec python "$PWD/scripts/codegen.py" --plugin ${plugin}/bin/artisan-capnp-codegen \
      --output "$PWD/modules/protocol/src" "$@"
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
    # Cargo.toml owns lint policy; match scripts/check.py on every platform.
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
      python ${src}/scripts/test_remote_host.py --bin-dir ${development}/bin --helper ${hostLauncher}/bin/forge-host
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
          python scripts/check.py --tests-only --runner nextest --bin-dir target/debug
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
          python scripts/check.py --tests-only --runner nextest --package artisan-packaging --bin-dir target/debug
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
      nixfmt --check ${../flake.nix} ${./workspace.nix} ${./forge-service.nix}
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
  packages = {
    default = release;
    inherit release development performance;
    dev-dependencies = devDeps;
    release-dependencies = releaseDeps;
    performance-dependencies = performanceDeps;
    editor = binary "release" "artisan-frontend" "editor";
    forge = binary "release" "artisan-backend" "forge";
    ae = binary "release" "artisan-editor-cli" "ae";
    installer = binary "release" "ae-installer" "installer";
    payload-manifest-generator = generator;
    release-tool = releaseTool;
    capnp-codegen = plugin;
    generated-bindings = bindings;
    dev-launcher = launcher;
    forge-host = hostLauncher;
    screen-demo = binary "dev" "artisan-screen-demo" "screen-demo";
    parity-visual-proof = parity;
    nix-payload = archive;
    unsigned-release = unsignedManifest;
    # Registration data and path list for the complete Nix runtime closure.
    inherit closure;
    forge-container = pkgs.dockerTools.buildLayeredImage {
      name = "artisan-forge";
      tag = version;
      contents = [
        (binary "release" "artisan-backend" "forge")
        pkgs.git
        pkgs.getent
        pkgs.coreutils
        pkgs.cacert
      ];
      config = {
        Entrypoint = [ "${binary "release" "artisan-backend" "forge"}/bin/forge" ];
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
    default = app editorApp "artisan-release";
    editor = app editorApp "artisan-release";
    dev = app devApp "artisan-dev";
    performance = app performanceApp "artisan-performance";
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
            --program ${binary "dev" "artisan-screen-demo" "screen-demo"}/bin/screen-demo \
            --software-icd ${pkgs.mesa}/share/vulkan/icd.d/lvp_icd.${pkgs.stdenv.hostPlatform.parsed.cpu.name}.json "$@"
        ''
      )
    ) "artisan-capture-screen-demo";
    screen-demo = app (shellApp "artisan-screen-demo" [ ] (
      graphicalEnvironment
      + ''
        exec ${binary "dev" "artisan-screen-demo" "screen-demo"}/bin/screen-demo "$@"
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
