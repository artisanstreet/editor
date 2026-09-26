# Native frame measurements

Measure the stages Nix builds, never raw Cargo output: `nix run .#dev` (Debug)
or `nix run .#dev -- --production` (Production). Both stages share the
Production codegen (opt-level 3, fat LTO, one codegen unit, `panic = "abort"`,
mimalloc, x86-64-v3); Debug adds debug info, debug assertions, overflow
checks, and the GPUI inspector. Report which stage a number came from.


The top-right counter measures presentation submissions during scheduled
animation and shows IDLE between animations. Unpresented CPU draws do not
increase FPS. CPU measures the most recent GPUI draw. This is not a GPU
utilization or physical monitor-refresh measurement.

## Repeatable capture

Set `ARTISAN_FRAME_CAPTURE` to an absolute JSON output path before launching
the editor. The opt-in capture requests full-window redraws, warms up for
30 seconds, and records ten seconds of GPUI draw and platform submission
timings. It stops requesting frames after the capture. With the variable
absent, this code does no work.

Optionally set `ARTISAN_FRAME_CAPTURE_CHAT=1` to open the most recent settled
chat with a sent message through the existing sidebar navigation path. This
does not send a message or start a run. Otherwise the capture leaves navigation
to the user. Keep the same chat, window size, monitor, and visible panels for
comparisons, and avoid interacting during the measurement.

The JSON includes the GPU, viewport, route, raw frame samples, and recent
foreground task timings. Where platform readback is supported, a PNG with the
same stem records the final scene; otherwise `screenshot_error` reports why
no image was saved.
These files can contain private chat content; keep them local. Screenshot
readback and file writing happen after the measured interval. The output
directory must already exist.

`draw.ms` measures CPU-side GPUI drawing. `present.ms` measures time inside
the platform renderer's draw/submission call, including any waits there. It
does not isolate GPU execution time. `present.interval_ms` measures consecutive
submissions while animation is requested. Use the per-frame distributions and
matching scene images when comparing builds. A forced full-window redraw is
a rendering workload, not a simulation of wheel input or a measurement of
input-to-display latency.

## FPS limit

Settings → Appearance → Performance provides 30, 60, 120, 144, 165, 180,
240, 360, 500, and Unlimited. Changes apply to the current window immediately
and are saved in `ui/frame-rate-limit` under the resolved Artisan home.
Unlimited is the default. On Windows it disables the application cap and
monitor pacing, selecting Immediate presentation when supported or Mailbox
otherwise. Numeric limits retain monitor synchronization. A numeric cap above
the monitor refresh rate does not force extra redraws. Idle windows remain
event-driven; Unlimited does not create a permanent rendering loop.

The limiter preserves the requested average cadence when the cap is not a
divisor of the monitor refresh rate. Frame intervals still fall on display
ticks. GPUI's existing inactive-window and thermal throttles also remain in
effect. A failed save is shown in the settings section; the chosen limit
still applies for the current session.

## Resuming after idle

The first input event that changes the UI now immediately enables GPUI's
interactive presentation grace period. It previously required six such events
within 100 ms, which excluded sparse clicks and the start of wheel interaction.
The shared behavior applies across platforms: recently interacting with an
unfocused window also bypasses the background animation cap. The user FPS limit
and thermal throttling still apply.

The grace period expires one second after the last input that changes the UI.
It does not install a timer or request frames on its own; idle platforms can
still park when frame demand ends. Platforms already delivering frame callbacks
can continue presenting during that bounded period.

The FPS overlay also ends its sample when the final animation callback finishes
without a draw. This prevents an idle gap from depressing the next animation's
FPS readout. Gaps while animation remains scheduled still count as stalls.

Regression coverage exercises first-input response, expiry and resumption, the
explicit FPS limit, return to idle, and the final callback without a draw. These
are headless shared-rendering tests; physical Windows input-to-display latency
still needs verification on the affected desktop.

## Model picker redraws

The model picker retains its grouped catalog projection across animation frames.
The cache is keyed by harness, query, and selected model; replacing the catalog
invalidates it even when the revision string is unchanged. The preview reuses
that projection instead of sorting and cloning the full harness list again.
Interaction tests cover these invalidations and verify that harness transitions
stop requesting frames after settling. These tests do not measure Windows GPU
performance. The FPS overlay samples scheduled animation and shows IDLE between
animations, excluding gaps between independent event-driven redraws.

## Continuous scrolling capture

Combine `ARTISAN_FRAME_CAPTURE_SCROLL=1` and `ARTISAN_FRAME_CAPTURE_CHAT=1`
with the capture path to send bounded wheel input through the actual transcript.
The diagnostic temporarily uses a 1024×480 viewport, restoring its size afterward.
`ARTISAN_FRAME_CAPTURE_VSYNC=1` or `0` overrides synchronization for the capture
without saving a preference. Measured scrolling uses a timer to deliver input,
with no forced redraw loop. Reports include wheel count, scroll range and
per-event window focus. Zero scroll range is not a scrolling measurement.
Compare active-window intervals separately from background throttling.

## Windows scroll measurement, 2026-09-13

On the Radeon RX 9070 XT, the optimized build with VSync disabled completed
an active-window, ten-second wheel-only capture at 1024×480. It delivered
639 wheel events across 738 px of scrollable content, without forced redraws.
There were 1,845 presentations, about 184 per second. Presentation intervals
were 5.18 ms median, 8.58 ms at the 99th percentile, and 11.11 ms maximum.
CPU draw time was 2.73 ms median; renderer draw/submission time was 2.20 ms
median. This bounds the tested workload only, not every thread or interaction.

The Windows wake callback now schedules dirty windows directly. The paint
region is validated before rendering so requests raised during rendering
survive to the next frame. Throttled retries use one timer instead of polling.

## Windows stage measurement, 2026-09-25

Nix-built stages (`x86_64-pc-windows-gnu`, MinGW-w64), Radeon RX 9070 XT,
1024×720 on the new-thread screen, forced full-window redraws with VSync off
(`ARTISAN_FRAME_CAPTURE_VSYNC=0`), ten-second measurement:

| Stage | Window | CPU draw median / p99 | Submission median | Presentations |
| --- | --- | --- | --- | --- |
| Debug | active | 0.57 / 0.99 ms | 1.14 ms | 530 per second sustained |
| Production | inactive | 0.54 / 0.86 ms | 0.38 ms | 1.05 ms median interval within bursts (about 950 per second); throttled between them |

Debug assertions and overflow checks cost about 5% of CPU draw time. The
Production run could not take the foreground, so its sustained rate is
throttled by GPUI's inactive-window policy; compare draw times, not rates,
across those rows. A chat thread with history costs more: a Debug capture
on one measured 4.15 ms median draw while inactive.

`nix develop .#performance` provides pinned profiling tools. Supply an absolute `ARTISAN_FRAME_CAPTURE` path outside the store. See the [development runbook](runbooks/native-dev.md#desktop-and-performance-tools) for the launch and host requirements.

## Scroll following and tool-chain defaults

The historical Electron/Svelte source was inspected at
`733eed518bee9b3d9b17c7f1f2b5cbb99d5fed26`, immediately before the web code was
removed. The relevant files are `modules/frontend/src/routes/components/thread-workspace.svelte`,
`conversation-trace.svelte` in the same directory, and
`modules/frontend/src/lib/conversation/scroll-position.ts`.

Tool chains use `open_groups[id] ?? false` there. Native tool chains now also
start closed, including live and failed work. Explicit toggles persist, and
navigation to a particular tool row reveals its chain. The enclosing work
session retains its own disclosure policy.

The native follow tolerance already matches Electron: distance from the bottom
must be strictly less than `max(64 px, viewport height × 0.06)`. Native measured
size changes now emit an extent event, equivalent to Electron's ResizeObserver,
rather than impersonating a user scroll. Streaming, image loading and disclosure
layout can therefore request a bottom correction without turning following off.
Wheel intent uses its destination to apply the tolerance; intermediate smoothing
positions cannot re-enable following after the reader has scrolled away.

Automatic corrections pin directly after layout and yield to reserved turn
space above its 192 px floor (or the composer clearance when larger). This avoids
fighting the space that absorbs answer growth below an anchored sent turn.
Explicit jump-to-latest keeps its existing smooth motion and interruption path.
The historical renderer also applied a separate visual-only glide capped at
56 px; that CSS transform is not part of this native follow-policy change.

Regression coverage includes live/failed chains staying closed, explicit tool
navigation, streaming resize without detachment, wheel leeway before smoothing,
reserved turn space, and the existing host/controller and tolerance tests.
