# Editor and Forge flight recorder

Debug payloads from `nix run .#dev` include the local flight recorder and start
it automatically. Use the app normally. A stalled UI, stalled operation, busy
admission, failed request, or failed navigation saves a Chrome Trace Event JSON
file that opens directly in [Perfetto](https://ui.perfetto.dev/).

Press **Ctrl+Shift+F11** to save recent Editor activity immediately. Automatic
incidents retain two seconds after the trigger; normal exit saves another
snapshot. This works for detached Windows launches without stdout capture.

Files go to `%TEMP%\artisan-traces` on Windows and `$TMPDIR/artisan-traces`
on Unix, normally `/tmp/artisan-traces`. `ARTISAN_TRACE_DIR` selects another
directory. Editor and Forge write on their respective machines, so a WSL
Forge's files are on Linux. The newest sixteen JSON files per application are
retained across process restarts. `ARTISAN_TRACE=0` disables recording at launch.
Set these variables in the environment of the actual Editor/Forge process;
an already running Forge service does not inherit a new terminal environment.

## What to inspect

The history covers up to two minutes or 32,768 events, whichever fits first.
High frame rates shorten the retained time window. Traces contain:

- UI and transport heartbeats, with stalled/resumed breadcrumbs;
- GPUI CPU draws and renderer submission durations, dirty-to-draw latency,
  animation intervals, and foreground task polls over ten milliseconds;
- UI handlers, route mounting, snapshots, patch processing, and event batches;
- command queue depth, admission/rejection, queue waits, execution spans,
  producer-to-consumer flow arrows, and thread identities;
- correlated wire requests, response-family expectations, outcomes, and
  reconnect/subscription work;
- recent-thread waits, thread-switch generations/phases, project intake,
  navigation gates, pending unsubscribe, and connection holds;
- matching Forge request spans and event-loop stall reports.

Async spans have unique IDs and use `b`/`e`, so concurrent work and executor
migration do not produce falsely nested synchronous slices. Unfinished spans
are closed at the capture boundary with `unfinished=true`. A beginning that
aged out is marked `begin_truncated=true`. `dropped_records` and
`evicted_records` make recording loss explicit. Thread-switch state includes
its owning span ID; request spans include the command span that issued them.

For the delayed busy notification, search for `recent_thread.failed`, then
inspect `navigation.gates` immediately before it. The ten-second recent-thread
wait ends with that failure if navigation is still inadmissible. Follow the
switch phase, outstanding command, and request rather than inferring that the
clicked thread itself was busy.

Use `python3 scripts/trace.py summary TRACE.json` for a text report of the
longest operations and the last navigation gates. To combine local Editor and
Forge exports into one Perfetto timeline:

```sh
python3 scripts/trace.py merge combined.json editor-TRACE.json forge-TRACE.json
```

Matching `request_id` values connect processes. Timestamps use Unix
microseconds anchored to each process's monotonic clock. Different machines
need synchronized wall clocks for visual alignment; IDs remain useful when
their clocks differ. Merge remaps process IDs to avoid Windows/Linux PID
collisions and distinguishes process restarts. Overlapping captures are
deduplicated; real span completions supersede synthetic snapshot boundaries.

## Build-time removal

The shared `artisan-tracing` module has no default features. Without `enabled`,
it contains only macros that erase their arguments. It has no recorder,
serialization dependency, environment handling, filesystem writer, or watchdog.
Each application also gates all tracing fields, startup tasks, and adapters
behind its `flight-recorder` Cargo feature. Production's Nix feature list is
empty, so the tracing implementation is absent from the binaries.

For a Cargo diagnostic build, enable `artisan-frontend/flight-recorder` and
`artisan-backend/flight-recorder`. Build without either for the normal
production behavior. Cargo features are additive; deliberately enabling these
features on a custom release build deliberately includes the recorder.

## Cost and limits

UI and transport producers use bounded, nonblocking sends. History collection
and filesystem output run on separate threads. Saturation drops records rather
than waiting. Arguments accept only bounded scalar values. Instrumentation
records fixed operation names, opaque IDs, enum states, counters, and source
locations; it does not record prompts, thread titles, file targets, credentials,
URLs, snapshots, screenshots, or provider payloads. Files are local, and Unix
exports are created with owner-only permissions.

The UI watchdog triggers after 500 ms without a foreground heartbeat. A long
navigation/project/request span triggers after five seconds. Automatic
captures have a ten-second cooldown. The existing Forge watchdog triggers at
two seconds. Whole-process suspension is reported separately by the watchdogs.
Recording is instrumentation, not native stack sampling: an uninstrumented
blocking function may still need an OS profiler. Completed GPUI task records
can attribute a slow poll to its source location after the UI resumes.

File writing never blocks the UI while the application runs. Normal process
exit joins the recorder and writer to finish the final file; force-killing the
process can lose the latest in-memory history. Export failures go to stderr.
The recorder does not require a GUI viewer to be open and makes no network
requests.

## Verification

```sh
cargo test -p artisan-tracing --features enabled
cargo test -p artisan-tracing --no-default-features
python3 tests/tracing/merge.py
```

Set `ARTISAN_TRACE_KEEP_FIXTURE=1` on the enabled test command and add
`-- --nocapture` to retain its generated stall, manual, and shutdown captures
for an importer check. Handoff arrows use one-microsecond endpoint markers;
their duration represents a marker rather than measured application work.
