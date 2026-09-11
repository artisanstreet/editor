# Parity visual-proof reference (worker lane)

Scope: `NATIVE_VISUAL_PARITY_TASKLIST.md` visual-verification row only.
No product, manifest, Svelte, vendor, backend, or generated file was touched.
New files in this packet: `modules/frontend/src/parity_visual_proof.rs`
(registration pending, see below) and this note.

## Production composition the fixture mounts (all read, none edited)

| Reference (Electron/Svelte, read-only) | Native owner mounted by the fixture |
| --- | --- |
| `routes/components/thread-workspace.svelte` transcript column | `ConversationHost` + `ConversationSurface` via `ThreadScreen::mount_proof` (shell lane `ff05bb1f`; this branch reads the pre-shell `mount` shape, root integrates) |
| `thread-panel.svelte` inspector column geometry | `desktop_shell` sidebar reservation, `DESKTOP_SIDEBAR_WIDTH_PX = 218.0` (`desktop_shell.rs:49`) |
| titlebar strip | `desktop_shell` titlebar reservation, `DESKTOP_TITLEBAR_HEIGHT_PX = 48.0` (`desktop_shell.rs:47`) |
| shell background continuity | `desktop_shell()` root gradient face (`desktop_shell.rs:134`), `DesktopTheme::neutral_dark()` (`modules/ui/src/theme.rs:1125`) |
| composer dock | `NativeComposer` mounted inside `mount_proof` |
| gate precedence opened > loading > failure | `ThreadScreenGate::Open` set inside `mount_proof` (shell lane `ff05bb1f`) |
| live thread title | `mount_proof` title param (`"Parity proof thread"`) |
| responsive inspector fit/hide | `set_content_width(actual window − rail)` + `thread_inspector_visible` per capture; 806px content hides, 1318px shows |
| caption height | shipping `TitlebarOptions { appears_transparent: true }` (`native_application.rs:8738`), hidden + unfocused for capture |

Fixture slots the wrapper requires but no lane owns for this packet
(identity / search / sidebar content) are empty `div`s, marked
fixture-owned in code. Sidebar *content* is not reference-covered; sidebar
*geometry* (218 px reservation) and composer containment inside the
conversation column are.

## Capture APIs found in canonical vendor (read-only)

- `Window::render_to_image` (`vendor/gpui-ce/crates/gpui/src/window.rs:2472`,
  `cfg(any(test, feature = "test-support"))`) delegates to
  `PlatformWindow::render_to_image`.
- Windows DirectX path exists: `WindowsWindow::render_to_image`
  (`crates/gpui_windows/src/window.rs:1139`) over `DirectXRenderer::render_to_image`
  (`crates/gpui_windows/src/directx_renderer.rs:638`): offscreen CPU image,
  staging texture, BGRA-to-RGBA, **no presenting, window need never be
  shown** (doc comment at line 632).
- `gpui_platform::current_headless_renderer()` returns `None` on every
  non-macOS platform (`crates/gpui_platform/src/gpui_platform.rs:83`), so
  the `HeadlessAppContext` screenshot route is macOS-only.
- Shipping configuration is `gpui_platform` with `wgpu`, under which the
  window renderer is `WgpuRenderer` — and `WgpuRenderer` has **no**
  `render_to_image` method, while `WindowsWindow::render_to_image` calls the
  DirectX signature unconditionally. Per root direction this is owned by the
  separate capture lane (`wt-parity-gpui-capture`); the fixture assumes
  `Window::render_to_image` works and performs **no** DirectX fallback as
  proof.

## Fixture invocation (root-owned steps marked)

1. Root registers the module (`mod parity_visual_proof;`, feature gate at
   root discretion) and enables `test-support` on the workspace
   `gpui_platform` dependency plus binary/export and Bazel wiring.
2. Root runs one process per capture (10 states × narrow/wide = 20
   sequential invocations; GPU/RAM reclaimed between — the all-at-once
   shape exhausted RAM and is gone):
   `parity-proof --case <slug> --viewport <narrow|wide>`.
   Slugs: `empty thinking working streaming completed error longform
   reference-settled reference-thinking reference-navigator`.
   Anything else (missing, reordered, extra, unknown slug/viewport) fails
   closed with usage on stderr and opens no windows.
3. Each process: shipping boot parity (`.with_assets(CatalogAssetSource)`
   + `register_bundled_fonts`), mount + seed through the controller, open
   one hidden unfocused window with the shipping transparent caption, then
   poll up to 50 × 100ms: `Window::resize` to the requested size (the
   `show: false` open leaves CW_USEDEFAULT), re-read platform bounds,
   `bounds_changed` to sync scale/viewport, and only after the size is
   valid publish the live content width, `Window::draw` (no present),
   `Window::render_to_image`, `ArenaClearNeeded::clear` on the same
   context, save
   `parity-proof-{case}-{viewport}-{logical}-scale{measured}-{WxH}.png`,
   quit. Terminal paths: seed/open failure quits immediately; capture
   ok/error settles and quits; resize exhaustion fails loudly. No
   internal timer (it cannot interrupt a blocked UI thread); root's
   external 45s guard owns the timeout.
4. Expected physical sizes at the 125% reference scale: narrow 1280x900,
   wide 1920x1125. The fixture asserts `logical * measured scale` from
   `window.scale_factor()` and fails loudly on mismatch instead of claiming.
5. Each capture publishes `geometry … requested=… actual=… scale=…
   sidebar=218 titlebar=48 content={actual window - rail}
   inspector={shown|hidden} title="Parity proof thread"`, a `manifest …`
   line (published title, composer attachment count read back from the
   live entity, projected block order — draft content has no
   production-visible accessor outside tests, so it is omitted rather
   than faked), and a `paint … quads=N` line. Zero painted quads fails
   the capture outright: no image is accepted from dimensions alone.
   Requested is never labeled actual.

## Reference complaint cases (user report, local only)

Reference (separate checkout, read-only):
`modules/frontend/src/routes/components/conversation-work-session.svelte`
(session item with `reasoning_summary`, settled durations, engine
attribution), `conversation-message.svelte` (no emoji pipeline — unicode
passes through as plain text), `lib/conversation/store.ts` +
`activity-status.ts` (session derivation, thinking words, settlement).

- `reference-settled`: user `Whoopty`; settled reply
  `Whoopty! 😄 Whats up?` (Final, Completed, attributed run);
  Reasoning fact `Planning a playful response.` with the same run;
  Completed turn with a 6s own span. Exercises single-run session
  grouping from the real pipeline (assistant provenance plus
  run-attributed reasoning — no producer emits `WorkSession` markers,
  so the fixture carries none), and `ThoughtFor{6000}` settlement.
  Tests assert the session group, its run, the 6s label/narration, and
  the byte-exact emoji reply.
- `reference-thinking`: user `Whoopty`; Active turn; Reasoning fact
  ``Checking `mood` for **playful** *tone* before replying.`` with the
  run. Exercises the live thinking summary line with inline code,
  strong, and italic fragments. Tests assert the session run, the exact
  summary body, and `Thinking` narration.
- `reference-navigator`: same settled Whoopty scene plus a second
  genuine Completed exchange (`Whoopty again` / `Still here and
  playful.`) on its own turn with globally unique ordinals, so the rail
  carries more than one user marker (a single marker renders collapsed
  per the marker policy). At capture the fixture focuses the first
  rendered navigator control through real window focus
  (`navigator_focus_handle`), asserts more than one candidate marker
  and a rendered handle, and fails rather than capturing a collapsed
  rail. Wide + narrow readbacks show the expanded rail above the
  composer.

Generic unattributed facts cannot trigger session grouping (runs are
never parsed or defaulted: zero or several content runs keep the legacy
layout), which is why these cases carry explicit run provenance.

Manual `RegisterTurn`/`Turn` driving is gone. Each case dispatches a domain
`SnapshotReceived` and, where the state needs one, directly registers
Activity/Reasoning/Error facts (`SceneFactCommand::Register`) — the
delivery-owned turn sync derives the drive
(`evidence/parity-projection-interface.md` in the projection lane): live
`Streaming`-lifecycle reply text yields streaming, Activity/ChangedFiles
facts yield working, Reasoning facts yield thinking, terminal turn
lifecycles settle on their own `updated_at` span. All turn/item times are
current-relative (`SystemTime`, never 1970) so the timed host clock renders
live spans. A controller refusal is recorded against its case; nothing
paints a hand-built scene.

## Exact support requested from root

1. Module registration (`mod parity_visual_proof;`, feature gate at root
   discretion) + `test-support` enablement on the workspace
   `gpui_platform` dependency + binary/export and Bazel wiring.
2. Capture lane: shipping-wgpu `Window::render_to_image` readback (capture
   worker reports GREEN in primary vendor `7aaf67d755`; this fixture calls
   the API and performs no DirectX fallback).
3. Real sidebar-slot content for full containment pixels (currently
   fixture-owned empty `div`s; content owned by `NativeApplication`,
   private). Geometry (218 px reservation) is production regardless.

## Known limits (no pixel claims from source alone)

- Nothing here renders until root registers, enables, and runs: the full
  reference comparison remains outstanding, and no PNG is claimed from
  source alone.
- Active states depend on the projection lane's delivery-owned sync being
  integrated alongside this fixture; before that they project Quiet and the
  runner records (not hides) the shortfall per case.
- Requested window bounds are not applied by a `show: false` open
  (CW_USEDEFAULT); the fixture drives `resize` + bounded settle and fails
  when the size never lands. Actual image dimensions must equal
  requested × measured scale — mismatch stays FAIL, never rescaled.

## Electron isolated render without user data / new dependencies

No Playwright/Storybook/screenshot harness exists in `modules/frontend`
(`package.json` scripts: build/dev/fmt/lint/test only; vitest present, no
browser runner). The dependency-free fixture dataset
(`src/lib/runtime/fixtures/`, e.g. `support.ts` with `FixtureReceipt`,
`FixtureFailure`, `FixturePreviewTarget`) is Effect-based and has no
component-render entry; its native counterpart
(`runtime_fixture_support.rs`) is pure policy. So: no existing isolated
Electron component render workflow was found — nothing to reuse, nothing
added (new dependencies forbidden). Native-side proof goes through the
fixture above; Electron reference stays read-only source.
