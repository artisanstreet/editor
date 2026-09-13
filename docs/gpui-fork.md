# GPUI fork (`artisanstreet/gpui-ce`)

The native editor renders through the Artisan fork of GPUI CE, not crates.io
`gpui`. This document is the model of record for that dependency.

## Dependency

`Cargo.toml` declares a git dependency pinned to an immutable revision:

```toml
gpui = { package = "gpui-ce", git = "https://github.com/artisanstreet/gpui-ce", rev = "<40-hex>", features = [...] }
gpui_platform = { package = "gpui_ce_platform", git = "https://github.com/artisanstreet/gpui-ce", rev = "<same>", features = ["wgpu"] }
```

Bazel resolves the same packages through `crate.from_cargo` in `MODULE.bazel`;
`@crates//:gpui-ce` is the canonical label (the versioned
`@crates//:gpui-ce-0.2.2` alias must not be referenced from BUILD files).

There is no `vendor/gpui-ce` submodule any more. A path dependency could not
be resolved by crate_universe across workspace boundaries, and a pinned
submodule gitlink hides whether the revision exists on the remote. The git
dependency is the single form Cargo and Bazel both understand.

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

The editor repository carries no GPUI `.patch` file and no
`crate.annotation(patches = ...)` for it. If a fix touches GPUI, it goes to
the fork and the editor bumps the revision.

## Bump procedure

1. In the fork: merge `origin/main` into `artisan/editor`, resolve conflicts,
   run the fork's tests, push the branch.
2. In the editor: update `rev` on both deps in `Cargo.toml`, refresh
   `Cargo.lock` (a plain `cargo metadata` is enough), then
   `CARGO_BAZEL_REPIN=true bazel build --nobuild //...` and commit both
   lockfiles.
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
controls. Ctrl+Shift+F12 toggles it. FPS counts actual GPUI redraws over the
recent one-second sample; FRAME is their mean interval, and CPU is the last
CPU draw duration. It does not measure GPU completion or displayed frames.
The counter does not request redraws, so idle windows retain the last reading;
a pause longer than one second resets the FPS sample on the next redraw.
Use PresentMon against editor.exe for independent presentation/display timing.
