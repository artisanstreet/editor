# GPUI fork (`artisanstreet/gpui-ce`)

The native editor renders through the Artisan fork of GPUI CE, not crates.io
`gpui`. This document is the model of record for that dependency.

## Dependency

`Cargo.toml` declares a git dependency pinned to an immutable revision:

```toml
gpui = { package = "gpui-ce", git = "https://github.com/artisanstreet/gpui-ce", rev = "<40-hex>", features = [...] }
gpui_platform = { package = "gpui_ce_platform", git = "https://github.com/artisanstreet/gpui-ce", rev = "<same>", features = ["cosmic-text"] }
```

Cargo resolves the pinned revision directly; `Cargo.lock` records its dependencies.
The fork is fetched from Git and does not require a vendored submodule.

GPUI selects the platform implementation at compile time:

| Platform | Renderer |
| --- | --- |
| Windows | Native D3D11 with Cosmic text shaping by default. The `windows-wgpu` Editor build feature selects the retained WGPU alternative with the same text system. |
| Linux | WGPU through the existing X11 or Wayland platform, with Vulkan and OpenGL backend support. |
| macOS | The existing native Metal renderer through `gpui_macos` and `gpui_apple`. |

The D3D11 default change applies to Windows. The macOS platform and Apple
renderer files are unchanged between the original `124c65d4` pin and `ec488d2`.
The combined reform worktree pins published main commit `f82ca3fe86540bc1d96acd1e295fefe4e8e7529e`, which merges the optimization revision `ec488d2` and retains main's existing blur example/filter helpers. Historical performance measurements below apply to the tested optimization revision. Shared GPUI changes still require platform validation. Windows appearance and
memory results do not establish macOS or native Linux behavior or performance;
the Linux runtime measurements so far use WSL, and this optimization work has
no macOS build or runtime qualification. See `docs/gpui-performance.md` for
the measured scope. The Windows backend switch preserves backdrop blur and
mesh rendering.

## Fork branches

| Branch | Role |
| --- | --- |
| `main` | default branch containing the published Artisan fork, including editor optimizations |
| `artisan/editor` | integration branch for editor patches before merging into `main` |
| topic branches | short-lived; integrated and validated before merging into `main` |

The local checkout is `../gpui-ce` next to this repository, on `main`.

The editor pins immutable revisions published on `main`. Never pin a revision
that is not pushed to the fork: the pin must resolve for every clone and for CI.
Existing pins remain valid when their commits are merged into `main`.

## Where patches live

All GPUI changes are commits in the fork, never patch files here:

- Runtime window FPS limits: `set_max_frame_rate` gates redraws over the
  display clock, retains fractional cadence, and discards missed deadlines.
  Removing the limit preserves monitor synchronization. Mandatory platform
  presentations still bypass the user limit.
- Windows wgpu frame pacing: per-window DXGI vertical-blank clocks follow
  the monitor containing the window. Coalesced frame requests park the clock
  when idle; active rendering uses vertical blank or a bounded 60 Hz fallback
  when the output is unavailable. Hidden/minimized windows pause the clock.
  Input, animation, active touchpad gestures and recovery re-arm it.
- wgpu renderer: sRGB/Oklab gradient encoding, ordered Bayer dither,
  scene-texture readback for hidden-window captures;
- native color emoji: fallback formation probing, whole-grapheme routing,
  `SegoeUIEmoji`/Apple face precedence;
- glass composition: outer shadows excluded from translucent samples.

The editor repository carries no GPUI `.patch` file for it. If a fix touches GPUI, it goes to
the fork and the editor bumps the revision.

## Bump procedure

1. In the fork: integrate `origin/main` and the editor patches on
   `artisan/editor`, resolve conflicts, and run the fork's tests. Merge the
   validated changes into `main` and push `main`.
2. In the editor: update `rev` on both deps in `Cargo.toml`, refresh
   `Cargo.lock` with `cargo update -p gpui-ce`, then run
   `cargo check --locked --workspace --all-targets` and commit the lockfile.
3. Verify that both manifest revisions and the lockfile's fork sources match
   the pushed revision. Confirm it is reachable from the published `main`
   branch, whose head is reported by
   `git ls-remote https://github.com/artisanstreet/gpui-ce refs/heads/main`.

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

Use `nix develop .#maintenance` for Python and `gh`. This checkout no longer
contains the earlier `verify-gpui-pin` app or PowerShell verifier. Performance
work uses direct Cargo build/check commands and separate executables; it
does not invoke the `dev` app, which manages the installed Forge and Editor.

The Windows Cargo shell uses the same GNU linker as release builds. LLD can
produce an invalid import table when this dependency graph combines two
versions of the Windows import libraries. A minimal C reproducer and the
Editor both leave `NtMapViewOfSection` and `NtUnmapViewOfSection` outside the
descriptor Windows binds. GNU links the same inputs correctly. Check a Windows
artifact with `python3 scripts/check-pe-imports.py /absolute/editor.exe` before
launching it. This checks static descriptor coverage, not DLL availability or
application behavior. Reproduction evidence is in
`.dist/performance/windows-import-repro` and `native-integrated-v1`.
