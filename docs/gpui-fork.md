# GPUI fork (`artisanstreet/gpui-ce`)

The native editor renders through the Artisan fork of GPUI CE, not crates.io
`gpui`. This document is the model of record for that dependency.

## Dependency

`Cargo.toml` declares a git dependency pinned to an immutable revision:

```toml
gpui = { package = "gpui-ce", git = "https://github.com/artisanstreet/gpui-ce", rev = "<40-hex>", features = [...] }
gpui_platform = { package = "gpui_ce_platform", git = "https://github.com/artisanstreet/gpui-ce", rev = "<same>", features = ["wgpu"] }
```

Cargo resolves the pinned revision directly; `Cargo.lock` records its dependencies.
The fork is fetched from Git and does not require a vendored submodule.

## Fork branches

| Branch | Role |
| --- | --- |
| `main` | upstream CE mirror; fast-forward only |
| `artisan/editor` | long-lived integration branch: upstream `main` + editor patches |
| topic branches | short-lived; merged into `artisan/editor` |

Only `artisan/editor` is pinned. Never pin a revision that is not pushed to
the fork: the pin must resolve for every clone and for CI.

## Where patches live

All GPUI changes are commits in the fork, never patch files here:

- Runtime window FPS limits: `set_max_frame_rate` gates redraws over the
  display clock, retains fractional cadence, and discards missed deadlines.
  Removing the limit preserves monitor synchronization. Mandatory platform
  presentations still bypass the user limit.
- Windows wgpu frame pacing: per-window DXGI vertical-blank clocks follow
  the monitor containing the window; FIFO presentation and a coalesced
  render permit prevent catch-up bursts. Hidden/minimized windows pause
  their clock. Unsupported or disconnected DXGI outputs retry with a
  bounded 60 Hz fallback. Headless rendering bypasses the frame gate.
- wgpu renderer: sRGB/Oklab gradient encoding, ordered Bayer dither,
  scene-texture readback for hidden-window captures;
- native color emoji: fallback formation probing, whole-grapheme routing,
  `SegoeUIEmoji`/Apple face precedence;
- glass composition: outer shadows excluded from translucent samples.

The editor repository carries no GPUI `.patch` file for it. If a fix touches GPUI, it goes to
the fork and the editor bumps the revision.

## Bump procedure

1. In the fork: merge `origin/main` into `artisan/editor`, resolve conflicts,
   run the fork's tests, push the branch.
2. In the editor: update `rev` on both deps in `Cargo.toml`, refresh
   `Cargo.lock` with `cargo update -p gpui-ce`, then run
   `cargo check --locked --workspace --all-targets` and commit the lockfile.
3. Run `scripts/verify-gpui-pin.ps1` before pushing.

## Local development against the fork

Clone the fork next to the editor and use a local, uncommitted
`.cargo/config.toml`:

```toml
[patch."https://github.com/artisanstreet/gpui-ce"]
gpui = { path = "../gpui-ce/crates/gpui" }
gpui_platform = { path = "../gpui-ce/crates/gpui_platform" }
```

Never commit that patch block.

## Animation scheduling

Visual motion samples GPUI display-frame callbacks, using elapsed monotonic
time for progress. Durations remain time-based so moving between 165 Hz and
240 Hz changes smoothness, not animation speed. Shared wheel smoothing uses
an exponential elapsed-time response instead of a fixed fraction per frame.
Shimmer, spinner, hover, disclosure, and copy feedback already use GPUI's
frame-driven animation machinery. One-shot tooltip/cleanup delays and the
once-per-second elapsed-time label are not animation frame clocks.

## Frame counter

The editor enables GPUI's FrameRate overlay below the top-right window
controls. Ctrl+Shift+F12 toggles it. FPS counts GPU presentation submissions during scheduled
animation over the recent one-second sample; FRAME is their mean interval,
and CPU is the last CPU draw duration. It does not measure GPU completion
or physical monitor refreshes. When no animation frame is scheduled, it shows IDLE.
Resuming animation starts a fresh sample. Long gaps during pending animation
remain in the measurement, so real stalls are not discarded as idle time.
The counter never requests redraws itself.
Use PresentMon against editor.exe for independent presentation/display timing.

The pinned Nix maintenance command is `nix run .#verify-gpui-pin` from the repository root. It supplies Python and `gh`, and checks the current checkout pin against the remote integration branch at runtime.
