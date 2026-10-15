# GPUI performance work

The objective is the lowest attainable total resident memory, including shared
libraries, while preserving used features and appearance, with approximately
0% idle CPU and the completed conversation visible. On 2026-10-13, the user
confirmed this memory metric and narrowed acceptance to Windows for now.
On 2026-10-14, the user removed the original 25–30 MB target and requested
optimization until no further reduction is possible. The working objective
has no fixed memory threshold: identify the purpose and lifetime of retained
memory, remove avoidable storage and work, and measure each candidate against
unchanged visuals, used features, active responsiveness and idle CPU. An
unattributed allocation, an untested alternative, or an exhausted time budget
does not establish a minimum. A universal physical minimum cannot be proved
by finite benchmarks; report the measured result and remaining uncertainty.
Linux acceptance is deferred. Existing WSL results remain diagnostics and do
not establish native Linux performance.

The thread goal tool rejected a replacement goal because the previous goal
is unfinished and exposes no objective-edit operation. It remains active;
the user's updated objective above governs this work. The old numerical goal
has not been marked achieved merely to permit its replacement.

The resumed 2026-10-14 work added native allocation call-stack attribution in
an isolated D3D11 process. Its final 736-event trace had no overflow, recursion
losses or truncated stacks. Sixteen reservations had AMD driver callers and
were released with the device; their byte counts are specific to this probe,
not an attribution of Editor's historical private memory. A documented heap
cache optimization released only 28,672 bytes immediately and showed an
8,192-byte net decrease on those reservation bases at later snapshots. GPU
buffer contents survived the operation. No production heap-cache change was
made. Full evidence and coverage limits are in
`.dist/performance/native-allocation-callers-v1/RESULTS.md`.

The scoped Windows settings-factory candidate also completed a native
cached/scoped/cached ownership comparison. All three probes and 21 inspectors
closed successfully, and all returned settings matched. The same 33 modules
remained loaded; scoped lookup showed no retained-memory benefit and increased
the repeated-query time in this diagnostic. The host-pressure override makes
these observations ineligible for resident comparisons. The candidate was not
promoted. Evidence is in `.dist/performance/scoped-ui-settings/harness/RESULTS.md`.

The combined core/backbuffer candidate repeated all six model-control scenes
with exact pixels outside the existing FPS overlay. The earlier small chevron
difference did not recur. Its first completed normal-release idle sample read
66,053,170 mean resident bytes and zero CPU time over 59.987 seconds. Visual
and process checks passed, but host load reached 95% and available physical
memory fell below 2 GiB. This sample is ineligible for memory acceptance and
establishes no saving against the 66,694,832-byte qualified reference. Pointer
and lifecycle validation remain pending. Evidence is in
`.dist/performance/core-backbuffer-idle-v1/candidate-3/measurement/qualification.json`.

Appearance must remain identical to the existing app. Backdrop blur, gradients,
shadows, text, images and model rendering are acceptance requirements. A smaller
memory result from a renderer with visible differences remains experimental.
The experimental native Windows renderer painted outer shadows inside panels.
Clipping those interiors restores exact pixels in the checked flat panel regions.
Dedicated blur comparisons now pass against WGPU. Full Editor appearance and
interaction checks qualify 23 Windows scenes at the published 8346 pin.
The recoverable static-image change is now published at ec488d2 and pinned by
Editor, with its separate pixel and recovery checks described below. The
backbuffer experiment remains isolated. The latest fully qualified ordinary
working-set reference still uses 8346; no accepted whole-process memory delta
has yet been established for ec488d2.

Visual checks found that the conversation initially stayed blank until input
in both the original pinned fork and early optimized builds. The 40-row
settled log used by the samplers did not prove visible content. Conversation
measurements before round 7 remain provisional launch observations. Round 7
and later results identify their full screenshots and visual verification
explicitly; readback itself did not repair the earlier blank state.

The inherited-opacity cache fix now reveals the conversation before input
in Windows captures. Both initial screenshots are identical, and typing,
selection, scrolling and palette interaction finish with clean process exit.
A separate platform input-handler lifetime fix removes the shutdown entity
retention that typing exposed in both original and optimized builds.

Use a fresh fixture for each typing suite. Drafts are persisted by its server,
so a new Editor data directory alone does not reset conversation content.

## Measurement

Use release builds for acceptance. Record the executable hash, GPUI revision,
backend, GPU, window client dimensions, display scale, foreground state,
workload and elapsed sample duration. Report bytes alongside MiB to avoid
confusing decimal MB with binary MiB.

`scripts/measure-idle.ps1` reads an explicit Windows PID. It records working
set, private committed memory, window state and CPU time. On Linux,
`scripts/measure-idle.py` records RSS, PSS, private resident memory, swap and
CPU time from `/proc`. Neither script starts, stops or modifies the process.
CPU percentages use one logical core as 100%; machine-normalized percentages
are reported separately.

Total resident memory is the acceptance target. Count shared library pages
in Windows working set and Linux RSS. Private resident memory, PSS and Windows
private committed memory are diagnostic measurements, not substitutes for
the acceptance metric. GPU texture byte counts
are allocation estimates and cannot be subtracted directly from process RSS.
Do not trim the OS working set to manufacture a passing result.

The acceptance workload is `completed-conversation-v1`: 20 finished turns,
40 items, Rust code blocks, prose, lists and tables. The isolated fixture is
created by `cargo run -p ae --example idle_fixture -- /absolute/new/directory`.
It requires a new directory, binds an ephemeral loopback port, writes its own
Editor identity and host configuration, and prints its PID and deep link.
It does not manage any installed Forge service. Point a separate Editor at
the fixture's `editor` directory with `ARTISAN_EDITOR_DATA`, then open the
printed link. For Windows under WSL, copy that directory into a fresh Windows
temporary directory. Loopback forwarding must reach the fixture's listener.

Measure home separately as a diagnostic baseline. For each workload, warm up
for at least 15 seconds and measure at least 60 seconds. Compare the same
client dimensions and scale. Record foreground and inactive visible samples
separately; also test minimize/restore, resize, typing, scrolling, transient
blur, images, emoji and opening/closing a model before accepting a change.
Take screenshots after memory sampling, because readback allocates buffers.

## Initial observations

On 2026-10-13, the installed Windows development binary's isolated, unpaired
home screen measured 355,889,152–356,085,760 resident bytes and
472,252,416–472,506,368 private committed bytes over 30 seconds after 10 seconds
of warmup. No CPU ticks were observed during that interval. The window was
visible, inactive and not minimized, with a 1800×1125 physical client area
and DPI 120, equivalent to 1440×900 logical pixels.
This is a diagnostic sample, not the release conversation acceptance test.
Raw measurements and binary hash are in `.dist/performance/` locally.

## Controlled measurements so far

The Windows conversation samples below use the same fixture, 1600×1125
physical client pixels, DPI 120, inactive visible window, 20 seconds warmup
and 60 seconds sampling. RSS values are mean bytes rounded to the nearest
byte. These are observations on the current machine, not the target achieved.

| Build | Mean RSS bytes | Mean RSS MiB | CPU, one core |
| --- | ---: | ---: | ---: |
| Installed development baseline | 421,631,161 | 402.10 | 32.188% |
| Original pinned fork, release | 224,087,661 | 213.71 | 0% |
| Optimized release round 1 | 221,667,194 | 211.40 | 0% |
| Optimized release round 2 repeat | 206,241,523 | 196.69 | 0% |

The development/release comparison includes profile and graphics validation
differences. It does not isolate the fork changes. The fair original/round 2
release comparison saves 17,846,138 resident bytes, or 7.96%. About 8.3 MB of
that difference is driver DLL residency; private resident pages fall by
about 9.6 MB. Do not attribute the entire reduction to GPUI heap savings.

The first round 2 Windows conversation sample was rejected because the window
was minimized during sampling. Its repeat kept dimensions, scale, visibility
and foreground state constant. Home round 1 intermittently consumed a whole
core in an unnamed native thread while both the UI and display clock slept.
Round 2 home sampled zero CPU. Later round 4 conversation runs consumed
93–99% of one core despite waiting GPUI UI/display threads. The same behavior
also occurs in the original release fork's empty home window. The hot thread
starts inside `amdxc64.dll`; 40 instruction-pointer samples landed in
`ntdll!NtWaitForSingleObject`. A later register capture established a zero
timeout, with the immediate caller in `KERNELBASE!WaitForSingleObjectEx`.
This establishes nonblocking native polling, not the cause of the loop. Changing the frame limit, repainting and minimizing then
restoring the isolated child did not resolve it. Third-party DLL injection
was observed in hot and quiet runs; causation is unproven. These hot runs do
not establish idle CPU acceptance.

On Linux, an optimized release A/B at the same 1518×900 framebuffer measured
297,140,224 resident bytes with mimalloc defaults and 256,634,880 bytes with
its `no_thp` build feature. Both recorded zero CPU ticks over 60 seconds.
The feature avoids allocator requests for transparent huge pages on Linux
and is inert on Windows. These runs used llvmpipe software Vulkan under WSL;
they do not establish hardware-rendered Linux acceptance. Another run changed
to 3× display scale and is excluded from this comparison.

Opt-in `GPUI_RENDERER_DIAGNOSTICS=1` with
`RUST_LOG=info,gpui::renderer_memory=info` records adapter identity, GPU resource
sizes, atlas occupancy, compiled pipelines and GPU allocator blocks whenever
allocation state changes. It adds no timer. Windows round 2 requested 58.4 MB
of GPU resources, including 36 MB of path targets for five paths, 10.8 MB of
blur targets, 9.44 MB of atlas pages and 2.1 MB of instance data. Backend
allocator reservations were 159.4 MB for 73.3 MB live allocations. These GPU
figures include padding and staging differences and are not process RSS.

Round 3 Linux atlas requests fell from 9,437,184 to 1,900,544 bytes and backend
reservations from 62,914,560 to 42,859,520 bytes, but process RSS remained
approximately 256.6 MB. A GPU resource saving is not automatically a process
RSS saving. Round 4 bounds path targets to the actual batch geometry while
retaining four-sample antialiasing and screen-space effects. At the same
1518×900 framebuffer, Linux requested path bytes fell from 27,324,000 to
3,194,880. Total requested GPU resources were 13,841,616 bytes; backend heap
reservations were 20,971,520 bytes. RSS was 230,674,432 bytes with zero CPU
ticks over 60 seconds, versus 307,941,376 bytes for the original release.
Linux focus, visibility and logical dimensions still need independent
verification before acceptance.

Windows round 4 requested 18.53 MB of GPU resources and reserved 54.53 MB in
backend heaps. Its conversation samples had the native wait loop described
above, so their roughly 194–213 MB RSS is diagnostic only.

A frozen allocator A/B on Windows changed only the global allocator from
mimalloc to Rust's system allocator. Both had the native wait loop and
identical GPU allocations. Mean RSS fell from 206,037,395 to 202,673,370
bytes, while private committed memory fell from 274,850,195 to 242,224,346
bytes. The 3,364,025-byte resident saving is too small to justify changing
allocators without active-workload and Linux measurements. The production
allocator remains mimalloc.

A separate WSL test using Mesa's D3D12 OpenGL backend successfully used the
AMD GPU and measured 244,559,872 resident bytes with zero CPU over 30 seconds.
It required per-process `GALLIUM_DRIVER=d3d12` and
`MESA_LOADER_DRIVER_OVERRIDE=d3d12`. This diagnostic has not changed system
driver settings or the default renderer selection. A round 4 repeat using
only the GL backend measured 236,482,560 resident bytes and zero CPU over
60 seconds. Its resident AMD driver image was approximately 45.7 MB and LLVM
approximately 30.5 MB, separate from application heap allocations.

### Native launch-size series

The old Windows series resized the first window from its native 1518×900
logical size to 1280×900. Startup work before that resize varied. Subsequent
samples retain 1518×900 logical pixels, record that no resize occurred, and
must be compared within this new series.

Windows release round 6 measured 189,571,273 mean resident bytes,
268,180,228 private committed bytes and zero CPU ticks over 60 seconds with
DX12. At the same native launch size, the original pinned fork measured
224,943,322 resident bytes and 382,422,234 private committed bytes with zero
CPU ticks. The observed differences are 35,372,049 resident bytes and
114,242,006 committed bytes, subject to the visible-content qualification
above. Its Vulkan diagnostic, using the same executable, reached the AMD
RX 9070 XT successfully after providing the Win32 module handle. That launch
measured approximately 225.2 MB RSS and 97.15% of one core. The hot thread
started in `amdvlk64.dll`. Native polling therefore also appears under
Vulkan; it is not established as a DX12-only problem. Backend choice remains
unchanged in production.

The Linux X11 round 5 sample independently verified a stable, viewable
1898×1125 physical window at explicit scale 1.25, matching the Windows
physical size. RSS was 242,311,168 bytes with zero CPU ticks over 60 seconds.
X11 reported keyboard focus throughout, while Windows reported an inactive
window; those focus states remain distinct. This still uses llvmpipe under
WSL. The round 5 Wayland launch unexpectedly selected 3× display scale and
is excluded from comparisons with the earlier 1× samples.

Round 7 is the first Linux release sample with a complete post-sample
1898×1125 screenshot visually confirming the conversation, code, table and
empty composer. It measured 236,142,592 resident bytes and zero CPU ticks
over 60 seconds after 20 seconds warmup, at explicit scale 1.25. The window
remained visible, focused and geometrically stable. The requested pre-warmup
origin was 0,0; the window manager placed it at 358,239, fully on screen.
This verifies that workload on WSL X11 llvmpipe. It still exceeds the memory
target and does not establish native hardware-rendered Linux acceptance.
Artifacts are in `.dist/performance/linux-visible-conversation-round7/`.

With `WGPU_BACKEND=vulkan`, the same Linux executable used 210,796,544
resident bytes and 0.01 CPU seconds over a 60-second interval.
Its fresh fixture and full screenshot verify the complete conversation and
empty composer. This is 25,346,048 fewer resident bytes than the default
Vulkan-plus-GL run. Separate native allocation traces locate 29,806,583 fewer
live requested bytes, chiefly an unused GL context; selected lavapipe
allocation totals differ by only 32 bytes. The production selection change
must still be measured because discovering and then releasing GL can retain
allocator pages. Artifacts are in
`.dist/performance/linux-conversation-release-round7-vulkan-clean/` and
`.dist/performance/linux-native-heaptrack-round7-vulkan-clean/`.

Windows production round 7 now verifies visible transcript content and an
empty composer using a complete post-sample client screenshot. The stable
1898×1125 client was fully inside the monitor and inactive. Mean working set
was 188,859,845 bytes and private commit 268,056,005 bytes. CPU was 98.77% of
one core, so this run fails the idle CPU target. Twelve subsequent instruction
samples of the hot AMD thread landed in a zero-timeout native wait; the UI
thread was waiting normally. The cause of that native polling remains under
investigation. Artifacts are in
`.dist/performance/windows-conversation-release-round7-visible/`.

`scripts/run-linux-idle.py --xdotool /path/to/xdotool --x11-scale 1.25`
selects X11 only for the isolated child. Its sampler reads ownership, client
dimensions, position, focus and viewability on every sample. It does not
move the pointer or resize or focus a window. X11 viewability does not prove
absence of occlusion by other applications.

The Linux allocator A/B observed 225,148,928 resident bytes with mimalloc and
221,937,664 with the system allocator, both with zero CPU over 60 seconds.
Both used 1518×900, but startup glyph/cache occupancy differed slightly, so
the 3.2 MB difference needs controlled repeats before attributing it entirely
to allocator choice.

## Published checkpoint and ongoing validation

The separate fork checkout is `../gpui-ce-performance`, branch
`artisan/performance`. Initial checkpoint `3f17269e78c72613eea7ca3ac1d2ae5fccfac380`
has the original pinned revision `124c65d4a50ace08ec1c432c324677584367253b`
as its parent. Followup `cfd745a759e872fe7b79f36114aad0b0d7857153` releases
Linux clipboard payloads after ownership ends and guards delayed cancellation
events. The published and pinned revision is now
`859c9caa2aaacedb66f881cfa4336e2c7fce3d9a`, adding shared atlas sizing and
native blur allocation on demand. It does not switch the production renderer.
The lockfile matches the published revision for all 17 fork packages, removes
36 unused registry packages and updates no registry package versions.
Editor enables the lighter `frame-rate`
feature, uses intrinsic SVG dimensions without a layout raster, and disables
mimalloc transparent huge pages.

The checkpoint passes 383 core, 54 Linux and 81 GPU/text tests, library
Clippy, workspace formatting and both platform release builds. Five changed
Windows scheduling/notification tests also pass on native Windows. The
Windows library suite has one pre-existing US-keyboard assumption that fails
on this host's currency-key mapping; the clipboard test was excluded to
preserve the user's clipboard. Editor's initial pinned `cargo check --locked
--workspace --all-targets` and all three SVG tests pass. The clipboard followup
and working atlas changes pass 385 core, 58 Linux and 80 GPU/text tests and
strict library/test Clippy. The updated Editor workspace checks pass, including
the current allocation pin in
`.dist/performance/editor-allocation-pinned-workspace-check.log`.
Round 8 measurements below use the initial checkpoint; later diagnostic
experiments and uncommitted followups do not change those measured results.

The default Linux round 8 release with automatic backend cleanup measures
228,118,528 resident bytes and 0.01 CPU seconds over 60 seconds, with the full
conversation and empty composer visible. This is 8,024,064 fewer resident
bytes than round 7 default. Native traces confirm the unused GL allocations
are released. The GL library remains mapped after discovery, consistent
with its dependency's deliberate no-unload policy. No production allocator
trim was added; a separate one-shot trim experiment reclaimed under 1 MB.

The machine reboot did not remove the Windows DX12 driver polling. The unchanged
round 8 production binary again measured 188.95 MB mean resident memory and
62.69% of one CPU core over 60 seconds, with a stable inactive window. The hot
thread still starts in `amdxc64.dll`; the UI thread waits normally. The artifact
is `.dist/performance/windows-conversation-release-round8-postreboot-visible/`.

The native D3D11 experiments remain separate from production acceptance:

| Same idle conversation, native Windows experiment | Mean resident MB | Private commit MB | CPU seconds over 60 seconds |
| --- | ---: | ---: | ---: |
| Demand-driven clock | 83.81 | 196.06 | 0 |
| Same clock, smaller atlas pages and lazy blur groups | 84.45 | 171.25 | 0 |

These are decimal MB. The allocation change reduces private commit by 24.81 MB;
it does not show a resident-memory improvement in this pair. Four native WARP
resource tests pass for atlas reuse, filter nesting, group allocation/reuse and
resize. Neither binary yet has complete appearance parity. Artifacts are
`.dist/performance/windows-native-d3d11-demand-clock-postreboot-repeat-60s/`
and `.dist/performance/native-allocation/idle/`; native resource test results are
in `.dist/performance/native-allocation/native-tests/`.

A post-sample working-set inspection of the allocation experiment attributes
43.59 MB to mapped image pages, 39.34 MB to private memory and 1.47 MB to other
mappings. The image total includes 13.93 MB of the app and 4.77 MB of the AMD
D3D11 driver, followed by Windows libraries. This inspection is not atomic and
is separate from the sampler. Resident memory remains far above the target;
private commit and GPU allocations must not be substituted for resident memory.

The later native D3D11/Cosmic prototype corrects the appearance differences.
It uses the production text shaper and BGRA glyph coverage, the same sRGB
conversion and ordered gradient dither, and clips outer shadows out of panel
interiors. It also restores batch constants after blur and corrects alpha
accumulation for ordinary draws and blur composites. The conversation's text,
code and icons match the WGPU reference byte for byte; composer differences
are limited to 13 channels by one code. Nested translucent filter scenes,
background changes and odd-size resizes differ by at most one code, and the
filters-off reference matches exactly. These are diagnostic captures, with
artifacts under `.dist/performance/native-d3d11-diagnostic/`.

The same frozen release executable, without capture support, measures
79.82 MB mean resident memory with a dormant DirectWrite text system and
78.84 MB without it. Both record zero CPU time over approximately 60 seconds
after 20 seconds of warmup. The full conversation and empty composer are
visible, at the same 1898×1125 physical size, in both runs. Their transcript
and composer pixels match exactly. Private commit is 191.93 MB and 192.36 MB
respectively, so this pair does not establish a commit improvement. The binary
predates the final translucent-overlap alpha fixes and bounded path targets;
it is not final integrated acceptance. See
`.dist/performance/native-d3d11-diagnostic/cosmic-prototype/dwrite-idle-ab.json`.

A later fixed-source comparison builds both renderers with GNU, the production
mimalloc allocator and the same private fixture with its FPS overlay disabled.
After 20 seconds warmup, the native release averages 74,191,990 bytes total
working set and records no CPU time over approximately 60 seconds. WGPU averages
197.82 MB and uses 30.8125 CPU seconds, about 51.36% of one core. Its hot thread
starts in `amdxc64.dll`; the UI thread waits normally. The differing native
thread activity is a qualification on the memory comparison, and the module
entry point alone does not establish the cause of that activity.

Both windows remain visible and inactive at 1898×1125, DPI 120, with the same
position. Full screenshots show the completed conversation and empty composer;
only 36 channels differ, all by one value. Separate capture builds pass ten
conversation/input/palette scenes and six mesh-control scenes. This fixed
snapshot predates the final Taffy, DirectWrite exclusion, content-group release
and bounded-highlighting integration. The native result is progress, not the
25–30 MB acceptance target. Evidence is
`.dist/performance/native-integrated-v1/idle-pair-qualified.json`.

The reviewed renderer/resource checkpoint and blur followups are published as
`8346ff8b304f4c8b79545d518574a5ddc8db85e9` and pinned in Editor. Windows now
defaults to native D3D11 with Cosmic shaping; `editor/windows-wgpu` keeps the
alternative backend available. Linux retains WGPU. Native blur uses a separate
Clamp sampler and the same half-resolution pass bounds as WGPU. Across 36
AMD hardware comparisons, all blurred regions match exactly. Twenty-four full
frames match exactly; the other twelve differ only at two background pixels
outside the blur by one channel value. Both regressions fail before the fix
and pass afterward on AMD and WARP. Shader bytecode and full texture dimensions
are unchanged. Evidence is in `.dist/performance/native-blur-edge-parity`.

Cosmic text support can also compile without the WGPU renderer and its backend
dependencies. Linux, Web and Windows WGPU explicitly enable the renderer.
The native Cosmic, Windows WGPU and Linux configurations pass strict Clippy.
This dependency split has no measured runtime saving yet.

Matching production-v3 capture builds at that published pin now qualify 23
Windows scenes: conversation reveal, scrolling, palette filtering, mixed
Arabic/emoji/combining-mark input, selection, model keyboard/toolbar controls,
and pointer orbit/pan. Native D3D11 and WGPU differ by at most one channel value
outside the changing timing overlay. The model pixels match byte-for-byte in
all seven pointer frames. Orbit, pan, zoom and wireframe change the model;
reset restores its original pixels and following text/code stays unchanged.
All test children exit cleanly. The first keyboard run had inconsistent
hover/camera/scroll state and remains rejected; explicit pointer/focus setup
qualifies the repeat without changing either executable. Evidence is under
`.dist/performance/production-v3-capture/`, including the three suite
qualification files, `pointer-assertions.json` and
`keyboard-control-assertions.json`.

The immutable capture executable SHA256 hashes are:

| Variant | SHA256 |
| --- | --- |
| Published 8346 native D3D11 | `b00483c69ce751453b5e1c44c1ec2e892de8ff028c55771322f2a8e9403f5dc3` |
| Published 8346 WGPU | `0a7c3b23bf11ed0208f975bf84cc9394e5f54c71c48c0f627af21d681e2d1693` |
| Isolated bounded native blur | `7d2f61b25e1d5bcb7ab2881df015add96cde6855b5e5d86040cd3fd18ab72684` |

The bounded-blur candidate uses the same frozen Editor source and a separate
8346 fork snapshot with only the reviewed blur patch. Its fresh shader output
changes four blur entries; ordinary primitive and mesh bytecode is unchanged.
It preserves the same 23 scenes against native 8346: conversation and keyboard
captures differ by at most one channel value, and the qualified pointer repeat
is byte-exact outside the timing overlay. An earlier pointer launch had nine
two-code differences in the account chevron; the unchanged executable's repeat
and other suites match that chevron exactly. Both runs remain recorded.
Source, shader and binary hashes, opaque-alpha checks and raw comparisons are
in `.dist/performance/native-blur-v3-capture/visual-qualification.json` and
the accompanying manifests and `VALIDATION.md`.

Both native variants also pass owned-window resize from 1518×900 to 1200×720
logical pixels and back, followed by minimize/restore, at scale 1.25. Each
restores its entire initial RGBA image byte-for-byte, with no GPU errors or
shutdown leaks. Candidate and reference differ by only four channel values of
one in every lifecycle phase. The post-lifecycle ten-second diagnostics measure
66.994 MB total working set for the reference and 67.236 MB for bounded blur:
there is no demonstrated resident-memory saving. Settled private commitment is
107.62 MB versus 103.44 MB, about 4.2 MB lower. These capture-feature runs follow
PrintWindow calls and are not normal-release idle acceptance. Active GPU cost
and a qualified release measurement remain separate requirements before a
production decision. See
`.dist/performance/native-blur-v3-capture/lifecycle-qualification.json`.

A separate native candidate samples the swapchain backbuffer during blur,
removing the additional full-scene texture and final fullscreen copy. It keeps
the original shaders, blur scratch dimensions and content-filter targets.
The removed texture descriptor represents 8,541,000 bytes at 1898×1125, but
making swapchain buffers shader-readable can change driver allocations. No
net GPU or process-memory saving is established yet.

Its independent original-renderer comparisons pass 36 full-frame scenes on
each of AMD hardware and WARP. A second test uses visible owned windows with
both attached DirectComposition and HWND swapchains, changes the scene across
50 ordinary presentations per process, resizes the actual client area, and
minimizes/restores the window. All 192 matched-adapter submitted-frame captures
are byte-identical, and restored frames match their initial pixels exactly.
All eight children exit normally. Strict native Clippy also passes. Active
whole-frame cost and normal Editor working-set measurements remain pending.
The experiment and immutable original/candidate binaries are recorded under
`.dist/performance/native-backbuffer-srv`, especially
`matched-adapter-comparison.json` and `present-qualification.json`.

Another candidate retains compressed bytes for large static raster images
after their first atlas upload. It preserves the original image dimensions,
orientation and pixels, and reconstructs the decoded buffer only on a later
atlas miss. Ordinary scrolling retains the existing atlas tile. Fourteen
focused data tests pass on Windows and Linux. Ten Windows Editor captures
preserve the reference appearance; a separate two-window atlas test moves the
initial 846,400-byte decoded buffer, recreates the image after three explicit
tile removals and a second window's first upload, and produces six byte-identical
captures. These establish ownership and visual behavior, not a whole-process
memory reduction. Three normal release trials average 66,122,547, 66,042,402
and 66,343,516 bytes total working set, versus 66,641,647 bytes for the
same-source reference; every run records zero CPU over 60 seconds. All three
candidate trials fail the predeclared host-pressure guard of at least 2 GiB
available and at most 90% physical load: their maximum loads reach 93%, 97%
and 94%. Their observed 298,131–599,245-byte differences are therefore
diagnostic, with no accepted memory saving. The full conversation, code,
composer and project icon pixels match the qualified reference exactly; the
candidate whole frames differ by only four, three and one channel values of
one. The unchanged guard and separate visual/measurement eligibility flags
are preserved in each run. The five-file change is published at
`ec488d251230037c8afa206b8f358baa78c57f0c` and pinned by Editor on the basis
of verified decoded-buffer release, exact appearance and restoration tests.
This promotion does not establish a total working-set saving or target
acceptance. The four runtime files are byte-identical to the measured
candidate; the production patch removes only the ignored fixture timing test.
Evidence is in `.dist/performance/recoverable-static-images`,
`.dist/performance/native-images-v3-capture`,
`.dist/performance/native-image-atlas-probe` and
`.dist/performance/native-images-v3-release/idle-comparison.json`. The normal
image-only candidate hash is
`126b99a8ea6c77271ee9af55890978ea289b00a1f3245f866916b8ae90a7c2e1`.

The normal production-v3 reference release at the earlier published pin
`8346ff8b304f4c8b79545d518574a5ddc8db85e9` averages
66,694,832 bytes total working set over 59.99 seconds after 20 seconds warmup,
with zero recorded CPU time. The completed conversation and empty composer
remain visible, inactive and stable at 1898×1125, DPI 120. Its transcript and
code pixels are byte-identical to qualified v2; the composer differs in eight
channels by one value. Forty-four pixels differ across the whole frame, at
most two values per channel. All 26 shader modules are byte-identical to the
qualified v3 capture executable. This single run does not isolate an additional
memory saving from v2; the 25–30 MB target remains unmet. Evidence and executable
hash `2b6d0e59999541fc055b1e8ef46dc0c42aef3cbbf77ce44692f43b29d093d318`
are in `.dist/performance/production-v3/windows-idle/qualification.json`.

The ordinary working-set snapshot breaks down into 10,637,312 bytes of Editor
executable pages, 28,020,736 bytes of DLL and other image pages, 26,386,432 bytes
of private allocation regions, and 1,593,344 bytes of other mapped data. These
sum to 66,637,824 bytes. The separately queried process counter is 32,768 bytes
higher; neither snapshot is the 60-second mean. Image pages include resident
code and static data, not the whole files on disk. Private commitment and GPU
allocation estimates are not extra resident RAM to add to these figures.

The private category spans 260 allocation bases. Its five largest bases hold
16,498,688 ordinary resident bytes, 62.53% of that category. Their owners are
not established by this memory map. Similar region sizes in the standalone
device probes are tracing leads, not proof of ownership in this Editor process.
The separate Rust allocation trace reports requested bytes in an instrumented
process and cannot be subtracted from this resident total. Next attribution
work should trace native and small-only allocator origins and reconcile them
with resident pages, starting with these large regions. The 66.7 MB reference
is not a proven minimum, and the 25–30 MB target remains unachieved. Source
hashes, full region ranking and accounting checks are recorded in
`.dist/performance/windows-residency-explanation-v1/breakdown.json`.

A separate child-only trial of the unchanged production-v3 reference sets
`GPUI_DISABLE_DIRECT_COMPOSITION=1`. It preserves full visible content and
stable 1898×1125 geometry; transcript, code, composer and project icon pixels
are exact, with only three whole-frame channel values differing by one.
The owned window extended style changes from `0x00240100` to `0x00040100`,
removing only `WS_EX_NOREDIRECTIONBITMAP`, and remains stable during the run.
Ordinary working set averages 66,803,880 bytes with zero CPU over 59.97 seconds.
The trial fails the unchanged pressure guard at 95% load and 1,366,360,064
available bytes, so no causal memory saving is accepted. Its raw working set
is 162,233 bytes above the same-source reference, rather than reproducing the
large standalone composition-setup allocation difference. Production defaults
remain unchanged. Exact context, pixels, styles, runner hashes and separate
mapping counters are in `.dist/performance/native-hwnd-production-v3`.

The immutable production-v2 snapshot predates those last two fork followups.
Its Windows release averages 67,283,112 bytes total working set over 59.98
seconds after 20 seconds warmup, with the completed conversation and empty
composer visible at 1898×1125, DPI 120, inactive throughout. It records
0.109375 CPU seconds, approximately 0.18% of one core, in three brief increments.
Post-sample thread waits do not identify their cause. This is approximately
6.91 MB below the earlier controlled native run, combining several changes;
it does not isolate a single optimization's effect. The target remains unmet.
See `.dist/performance/production-v2/windows-idle/qualification.json`.

A later same-executable allocator comparison recorded 67,020,901 mean working-set
bytes with mimalloc defaults and 67,302,182 with child-only
`MIMALLOC_PURGE_DELAY=0`. Both recorded zero CPU time over 60 seconds after
20 seconds warmup. Focus and geometry stayed constant, and transcript, code
and composer pixels were byte-identical to the qualified v2 reference.
Concurrent WSL compilation was recorded. Immediate allocator decommit showed
no resident-memory benefit, so the default remains unchanged. These were
allocator operations, not OS working-set trimming. Evidence is in
`.dist/performance/windows-allocator-purge-v2`.

The first completed Linux production-v2 sample is excluded from idle acceptance.
Its window gained focus and showed transient additional content during a short
CPU burst. The sampler correctly marks window state unstable. Its 218.15 MB
mean RSS and 13.05% average CPU describe that interrupted run. CPU was nearly
flat before and after the activity. The renderer is Vulkan llvmpipe under WSL2,
so this also does not establish native hardware-rendered Linux acceptance.
Evidence and the focus timeline are under
`.dist/performance/production-v2/linux-idle-qualified`.

The no-DirectWrite run's post-sample inspection attributes 40.82 MB of its
working set to image pages, 36.45 MB to private allocations and 1.52 MB to other
mappings. The image total includes 12.57 MB of the executable and 4.77 MB of
the AMD D3D11 driver. This describes current costs, not a minimum possible
footprint. The 25–30 MB resident target remains unmet.

For that run, a separate `QueryWorkingSetEx` enumeration found 83,013,632
valid resident bytes, including 4,222,976 bytes of private write-combine pages
outside the ordinary working-set enumeration. The post-sample working-set
counter was 78,823,424 bytes. These reads are not atomic. The standard working
set remains the Windows acceptance metric; the broader enumeration is retained
as a diagnostic and is not silently mixed into the A/B results.

A separate optimized Linux build using the System allocator and line tables
allows heaptrack to identify Rust allocation owners. Its settled snapshot at
52 seconds contains 14,252,130 live requested bytes from executable-origin
stacks and 68,169,224 from native shared-library stacks. Two full-window captures
are pixel-identical and show the completed conversation and empty composer.
This diagnostic is an allocation trace, not a production memory acceptance run.
The trace is `.dist/performance/rust-heap-audit/profile-v3/`.

Syntax highlighting accounts for about 6.66 MB of those Rust allocations,
mostly compiled regular expressions. Mermaid's eager startup warmup accounts
for another 1,047,710 bytes despite the absence of diagrams. Editor now starts
that warmup only when preparing a Mermaid fence, including an open streaming
fence, and warms the same font family used by real diagrams. Previously the
warmup used a different family cache key. Diagram layout, colors and rendering
are unchanged. In the isolated Onig-plus-demand-warmup Editor trace, those
warmup allocations are absent. That combined trace is diagnostic, not an
isolated measurement of the warmup change's effect on RSS.

The unrestricted Onig highlighting experiment reduced requested allocations
but stalled on a roughly 49 KB unfinished C++ template expression. It is not
approved for production. A separate candidate limits each search and falls
back to the original fancy-regex implementation for a pattern that reaches
the limit. Its headless allocation trace retains 1,307,879 bytes, compared with
6,735,601 for the original engine. Its checked token output matches across
the grammar corpus, 213 Markdown documents and 25 additional cases, including
long malformed input and valid deeply nested C++ and Unicode strings. Actual
Windows execution also preserves the corpus and formerly slow C++ hashes.

A full Linux Editor comparison with the System allocator measured
204,685,312 bytes total RSS for the original and 195,497,984 for bounded
highlighting plus deferred Mermaid warmup. Both windows remained visible,
focused and stable at 1898×1125 throughout the 60-second samples. The original
used 0.01 CPU seconds; the candidate used 0.00. Both captures show the complete
conversation and empty composer. Transcript, code and composer pixels match
exactly; the differing channels are confined to the diagnostic FPS display.
This is a controlled diagnostic pair on WSL llvmpipe, not production or native
hardware-rendered Linux acceptance. See
`.dist/performance/syntect-audit/bounded-idle-comparison.json` and
`bounded-pixel-comparison.json` in the same directory.

The bounded full-process allocation trace retains 76,736,226 requested bytes,
compared with 82,421,354 in the original trace. Root Editor now uses scoped
Syntect and Onig source copies under `vendor/highlighting`, with unchanged
grammar assets, recorded upstream hashes and the original fancy engine as the
budget-limit fallback. Locked metadata selects a static bundled native engine
without bindgen. The Nix source filter includes the vendor directory and its
ancestors; an actual filtered store source was checked for both manifests,
modified runtime files and all four syntax/theme assets. Release archives
include the dependency notices. Production validation passes 90 Editor Markdown
tests, nine adapter/validation tests and compilation of all 3,732 unique stored
or expanded grammar patterns. An independent adapter review also added a safe
range check before native pointer arithmetic for invalid public search input;
the frozen production-v2 measurement binaries predate that defensive follow-up.
Compilation and finite token/capture comparisons do not prove the engines
equivalent for every possible input. Full production-allocator Editor memory
validation on both platforms remains pending.

The trace also identifies a redundant full-pixel-buffer clone when GPUI wraps
a decoded static image in a one-element SmallVec. Moving the frame into the
inline buffer removes that copy; the required decoded image remains retained.
The integrated fork passes 393 core, 58 Linux and 75 GPU/text tests, including
the path-target tests moved from the GPU crate into shared core code.
This is a peak-allocation and copying improvement, not removal of the image.

Taffy now enables only the layout facilities GPUI exposes: block, flex and
grid layout, plus its tree and standard-library support. It excludes unused
CSS float and calc support, detailed grid diagnostics and Taffy's separate
content-size output. GPUI computes content bounds itself. An isolated probe
produced identical geometry for 9,144 node layouts across 72 mixed cases.
At capacity 2,048, retained requested allocation falls from 2,430,112 to
2,364,544 bytes. All 526 integrated tests pass with this feature selection.
This is an allocation result; full Editor RSS and capture validation of the
post-snapshot feature change remain pending.

- Windows display clocks and X11 frame timers wake on demand and sleep when
  idle. Input, animation, touchpad inertia and GPU recovery must re-arm them.
- Path, backdrop blur and content-filter textures allocate only when needed.
  Model targets and geometry are released after the last model leaves the
  scene. Owned atlas uploads reuse their raster buffer.
- Optional path, blur and surface pipelines compile on demand. Scene replay
  stores indices into its typed arrays instead of duplicate draw records.
  ASCII shaping skips emoji-family loading and grapheme segmentation.
- Atlas pages grow from small initial pages and instance buffers start at
  256 KiB. Backend heap block sizes are capped independently of resource size.
  These latest allocation changes are still under measurement.
- Taffy 0.13 retains measurement contexts when its nodes are cleared. GPUI
  explicitly removes those contexts before invalidating node IDs, releasing
  captured text/layout data while keeping reusable node capacity. Destructor
  tests verify frame lifetime and release across node reuse.
- Explicit accessibility properties allocate storage only when set. The
  empty field shrinks from 328 bytes to one optional pointer. Click, focus
  and accessibility actions remain independent of that storage. The measured
  fixture has no explicit properties across 435 peak Interactivity objects;
  this is a layout saving, not a measured whole-process RSS saving.
- Plain text lines keep one decoration run inline; rich lines grow as needed.
  This replaces 32 inline runs per shaped line and a 32-run heap reservation
  per wrapped line. Tests preserve style merging, caching and splitting.
- GPU validation recovery preserves its failure count through skipped frames
  and device recreation. One resource reset is followed by bounded retries;
  successful presentation clears the count.
- Parsed SVGs expose their intrinsic size so layout can avoid rasterizing a
  full natural-size image solely to obtain its aspect ratio.
- Linux builds use mimalloc's `no_thp` feature after the controlled release
  A/B reduced resident memory by 40,505,344 bytes with zero measured idle CPU.
- A `frame-rate` feature keeps the FPS overlay without full profiler journals,
  histograms and input/task tracing. `profiler` still includes the overlay.
- Arenas allocate chunks on first use, skip destructor records for trivial
  values and reuse validity generations only after all handles are gone.
- Image decoding retains PNG, JPEG, GIF, WebP, BMP, ICO, TIFF and PNM. The
  default AVIF encoder, EXR, HDR, DDS, TGA, farbfeld, QOI and image's Rayon
  feature are excluded. SVG support remains. Advertised extensions match.

## Current isolated Windows experiments

The backbuffer experiment now passes the full 23-scene Windows appearance
suite against the frozen 8346 reference. The host-pressure guard passed
throughout and all three owned Editor children exited cleanly. Seven pointer
interaction images are exact outside the FPS overlay; the remaining sixteen
images differ by 31 individual channel values in total, each by one code,
all on text subpixel edges. Blur materials, icons, the composer, model controls,
scroll restoration and model reset remain intact. This capture build provides
appearance evidence only. Earlier standalone results reduced ordinary working
set by 110,592 bytes while reducing private commit by 9,023,488 bytes; those
metrics remain distinct. Full Editor resize/minimize/restore validation and a
normal-release memory comparison are still pending. Evidence:
`.dist/performance/native-backbuffer-v3-capture/RESULTS.md` and
`visual-review.json` in that directory. The change remains isolated.

The lazy drag-image helper passes five native COM sharing, retry and reentry
tests plus a two-window probe of the actual registered OLE drop callbacks.
The probe preserves Unicode paths, copy/unsupported effects, DPI conversion
and registration cleanup. The helper remains absent before the first drag
callback and is shared thereafter. OS drag-image appearance and the complete
Editor attachment flow still need validation. This candidate is not in the
production pin. Evidence: `.dist/performance/native-lazy-drag-v3/` and
`.dist/performance/native-lazy-drag-callbacks/RESULTS.md`.

The batched draw-constant candidate passes 15 native renderer tests on both
WARP and AMD hardware, with one pre-existing ignored shader-reference test.
All six indexed primitive kinds, intervening blur/path draws, page/cap
boundaries, large-to-small storage release and upload-failure recovery retain
exact pixels against the original upload path. Strict Windows Clippy passes.
Packing writes 256 bytes per draw and adds a batch traversal. Repeated matched
native tests reduced render-submission CPU time by 8.4–8.8% for the 19-draw
mixed scene and 41–46% for 257 draws. All matched pixels were exact; GPU and
Present benefits were not established. The memory pair showed 69,632 more
resident bytes for packing and failed the host-pressure guard, so there is
no qualified RAM saving. These are renderer tests, not full Editor results.
Evidence: `.dist/performance/native-batch-constants-v2/RESULTS.md` and
`.dist/performance/native-batch-diagnostic/RESULTS.md`.

An alternative source experiment supplies immutable instance indices through
the D3D11 input assembler. It removes per-draw constant uploads and the extra
batch traversal while keeping the six ordinary vertex shaders' floating-point
work unchanged. Both isolated Windows builds now pass, and the bytecode changes
are confined to the six intended vertex shaders; all twenty other shader
entries remain identical. The candidate passes thirteen native renderer tests
on each of WARP and AMD hardware, including buffer growth, reuse, release and
invalid-size rejection. Fourteen separately compiled reference images per
adapter match exactly, including nonzero instance ranges, blur, paths, mesh
interruptions and renderer recreation. This establishes renderer correctness
for those cases; full Editor appearance, speed and resident-memory effects
remain unmeasured. The candidate is not in the production pin. Evidence:
`.dist/performance/native-instance-indices/correctness-result.json`.

Five isolated core changes now pass 105 selected Windows regression tests:
reusing scene-sort storage, compact keybinding storage, removing the ICU
string-length import, removing disabled-profiler work, and reusing Cosmic
font-cache entries at the requested weight. The font tests compare layout
and raster output with the original handles, including a real fallback and
variable fonts. All local crates were rebuilt from the sealed source, and
all three test executables pass import validation without ICU. A separate
profiler-enabled build and exact task-wrapper test also pass. Strict Clippy
found one unnecessary clone in a keyboard test; replacing it with a borrowed
slice produced the v3 snapshot, with identical runtime source. Strict Clippy
and all 105 tests now pass on v3. Combined Editor validation is in progress.
These tests establish behavior, not a resident-memory saving. Evidence:
`.dist/performance/core-storage-v2/RESULTS.md`,
`.dist/performance/core-storage-v3/test-result.json` and
`.dist/performance/core-storage-v3-validation/result.json`.

The combined core-storage/backbuffer candidate now has frozen capture and
normal release builds. The normal executable is 30,395,904 bytes, SHA-256
`bf1903dbf925328fc17ffe470281199a7fa852f6ac825e06305eba182b0b19df`.
Its 26 shader entries are byte-identical to the original shaders. File size
is not a resident-memory measurement. Ten matched interaction scenes pass:
19 color-channel differences of one value across the entire suite, outside
the predeclared changing FPS overlay. Both binaries use the same fresh
fixture so their host-avatar seed matches. The completed thread starts read;
its conversation data and empty draft remain unchanged between the two runs.
Six model-control scenes subsequently captured successfully under the host
guard, but each has nine color-channel differences of two values on the
sidebar account chevron. They fail the strict one-value pixel tolerance and
remain unaccepted. A same-reference reproducibility comparison is prepared;
its first preflight failed before launching a child. Model-pointer scenes,
full Editor resize/restore and normal memory qualification are still pending.
Evidence: `.dist/performance/core-backbuffer-editor-v2/` and
`.dist/performance/core-backbuffer-visual-v4/attempt-1/interaction/`.

A separate compact Taffy-style proof passes strict Clippy and eight Windows
tests. Its owned representation is 248 bytes versus 552 bytes for the current
dense style. Tests preserve the numeric bits, getter values, supported enum
combinations, grid ranges and iterator behavior. Two earlier test-only defects
are retained with their failed build logs; v3 corrects them without changing
the representation. No production node uses this type yet. At the previously
traced 1,088 node slots, the projected style-storage reduction is 330,752
requested bytes; this is not a measured resident saving. A tree adapter and
solver-output parity are still required. Evidence:
`.dist/performance/compact-taffy-style-v3/test-result.json`.

A standalone D3D11 diagnostic created and released a device without an Editor
window or scene. Its resource-lifetime checks passed, but Windows memory load
reached 98%, so its memory totals are ineligible for acceptance. A separate,
already-built D3D11/D3D12 probe includes minimal command submission and
completion in both APIs. Its first guarded series stopped before launching
any child because host pressure rose again. No graphics API or device-flag
change has been integrated from these probes. Evidence:
`.dist/performance/native-device-flags/structural-default-1/` and
`.dist/performance/native-d3d12-creation/windows-series-1/`.

The same-binary D3D11/D3D12 operational probes subsequently completed all
11 checkpoints each, including command completion and release on one owner
thread. Both children and all 22 inspectors closed. D3D12's completed-submission
working set was 53,665,792 bytes versus 41,693,184 for D3D11. These runs used
the explicit diagnostic mode, and the D3D11 run reached 91% host memory load;
they are not qualified memory comparisons. This result provides no evidence
of a D3D12 saving and does not justify a renderer rewrite. Shaders, rendering,
swapchains and recovery are outside this probe. Raw stages and module versions:
`.dist/performance/native-d3d12-creation/operational-comparison-1.json`.

One-shot `IDXGIDevice3::Trim` did not release an allocation base or reduce
the recorded DXGI resource usage. Its three runs failed the existing host
pressure guard, and no Trim policy was integrated. Correctly releasing all
device interfaces did release substantial driver-associated storage and
unmap AMD driver libraries; the final cut also releases CPU scene data, so
its entire reduction cannot be assigned to the driver. Evidence:
`.dist/performance/native-device-trim/RESULTS.md`.

## Audit coverage

The initial fork inventory contains 424 files, including 309 Rust files and
about 6.55 MB of Rust and shader source. The local inventory records paths,
sizes and SHA-256 hashes. Inventory is not evidence that every file has been
reviewed.

Current detailed work covers renderer target lifetimes and uploads, Windows
and X11 scheduling, profiler dependencies, arena allocation, image codecs and
recoverable raster storage. Windows allocation tracing identifies the large
Rust buffers separately from native and mapped-image memory. Native device
creation/release, retained scene storage, startup services and import owners
have additional isolated investigations. These do not complete the review of
every file. Remaining work includes small-allocation ownership, text shaping
and glyph cache bounds, event dispatch, remaining asset/platform paths and
the full file-by-file audit. Source deletion or a smaller binary alone does
not prove lower running memory.

Build with Cargo through `nix develop .#cargo-linux` or
`nix develop .#cargo-windows`. Do not run `nix run .#dev` during this work:
it restarts Forge and would terminate the active session. Test processes
must have separate data directories and be stopped only by their exact PIDs.

## Validation record

The inherited-opacity and input-handler fixes pass 381 core, 54 Linux and
75 GPU/text tests. The later optional accessibility storage change passes
382 core, 54 Linux and 75 GPU/text tests, including click/focus actions without
explicit properties. Windows fixed capture produced 12 screenshots across
four isolated children, all exiting successfully. No-input initial reveal,
mixed Arabic/emoji/combining-mark text, selection, scrolling and palette
filtering were visually checked. A palette result extending below the panel
boundary remains a separate observation under investigation.

Linux can capture the exact production release with
`scripts/run-linux-idle.py --capture-with /usr/bin/import --xdotool /path/to/xdotool`.
Capture runs after memory/CPU sampling and checks the owned window PID.
Use `--window-origin 0 0` when needed to keep the complete client area on
screen; this changes only the child's position before and after warmup, with
a one-second settling delay before sampling. PNG dimensions
are compared with client dimensions, and visual verification is recorded
separately. Windows `-CaptureWindow` uses PrintWindow on the exact owned HWND
after sampling. Its production smoke reproduced the known blank state,
so it did not silently repair that defect before the image was saved.

Round 6 passed 377 core tests, 54 Linux platform tests and 73 GPU tests.
Coverage includes compact/full path pixel parity with storage and texture
instances at 128×128 and 173×117, including clipping, gradients, dither,
patterns and overlapping translucent paths. The full `profiler` feature
also compiles independently of the lighter `frame-rate` configuration.
Windows release round 6 builds successfully and both native graphics backends
launch. Native touchpad, notification and device-loss recovery integration
remain outstanding. Passing pure state tests alone does not establish them.

The first Linux scripted interaction run reached scrolling, the palette and
mixed-language typing, but Linux had no implementation of the test capture
API. The X11 bridge now uses the existing wgpu readback path under
`test-support`; the visual run must be repeated. That first test build also
reported a retained TextEditor handle on shutdown, which remains under
investigation. Its failure is not counted as visual acceptance.
