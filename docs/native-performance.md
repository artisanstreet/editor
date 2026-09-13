# Native frame measurements

Use `scripts/dev.ps1 -Performance` on Windows when evaluating animation or
scrolling. This selects Cargo's `performance` profile, which inherits `dev`
and enables optimization level 2. Debug symbols and assertions remain enabled.
The ordinary `dev` profile stays unoptimized for step-through debugging.

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
