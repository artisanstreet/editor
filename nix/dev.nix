{
  pkgs,
  graphicsLibraryPath,
  profile ? "debug",
  blazePackage ? null,
}:
let
  runner = pkgs.writeShellApplication {
    name =
      if blazePackage != null then
        "devfast"
      else if profile == "debug" then
        "dev"
      else
        "dev-${profile}";
    runtimeInputs =
      with pkgs;
      [
        git
        coreutils
        gnused
        gnugrep
        procps
        util-linux
      ]
      ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.systemd ];
    text = ''
      ARTISAN_DEV_GRAPHICS_LIBRARY_PATH=${pkgs.lib.escapeShellArg graphicsLibraryPath}
      : "''${ARTISAN_DEV_PROFILE:=${profile}}"
      ARTISAN_DEV_BLAZE=${if blazePackage == null then "" else "${blazePackage}/bin/blaze"}
    ''
    + builtins.readFile ./dev.sh;
  };
in
{
  type = "app";
  program = "${runner}/bin/${runner.name}";
  meta.description = "Build (${profile} profile) and install Forge, the Editor and ae${
    if blazePackage == null then "" else " with Blaze"
  }";
}
