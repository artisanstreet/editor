# Native frame measurements

Use `scripts/dev.ps1 -Performance` on Windows when evaluating animation or
scrolling. This selects Cargo's `performance` profile, which inherits `dev`
and enables optimization level 2. Debug symbols and assertions remain enabled.
The ordinary `dev` profile stays unoptimized for step-through debugging.

The top-right counter measures redraw cadence, not GPU utilization or display
refresh rate. Hovering occasionally includes the idle gaps between redraws in
the FPS/FRAME readings. CPU measures the most recent GPUI draw. Neither a low
idle FPS nor its change when the pointer moves establishes a rendering limit.

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
foreground task timings. A PNG with the same stem records the final scene.
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
