# `nix run .#dev`: the development loop.
#
# The app is the Linux dev runner itself (scripts/native_dev), not a
# wrapper around it. Run from the checkout, it builds the checkout's stage
# outputs with Nix and deploys both halves through the shipping installer:
# the Linux installation with its Forge as a systemd user service, and the
# Editor (inside WSL, the Windows build through interop), registered with
# that Forge and launched.
#
#   nix run .#dev                      Debug stage: build, install, launch
#   nix run .#dev -- --production      Production stage
#   nix run .#dev -- stage             install without launching the Editor
#   nix run .#dev -- where | prune     inspect or clean the installations
#   nix run .#dev -- --linux           the Linux Editor, also inside WSL
{ runner }:
{
  type = "app";
  program = "${runner}/bin/dev";
  meta.description = "Build the product with Nix and deploy the Forge service and the Editor as Artisan Street Dev";
}
