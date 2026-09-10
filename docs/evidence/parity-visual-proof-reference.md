# Parity visual-proof reference (worker lane)

Scope: `NATIVE_VISUAL_PARITY_TASKLIST.md` visual-verification row only.
No product, manifest, Svelte, vendor, backend, or generated file was touched.
New files in this packet: `modules/frontend/src/parity_visual_proof.rs`
(registration pending, see below) and this note.

## Production composition the fixture mounts (all read, none edited)

| Reference (Electron/Svelte, read-only) | Native owner mounted by the fixture |
| --- | --- |
| `routes/components/thread-workspace.svelte` transcript column | `ConversationHost` + `ConversationSurface` via `ThreadScreen::mount` (`modules/frontend/src/thread_screen.rs:287`) |
| `thread-panel.svelte` inspector column geometry | `desktop_shell` sidebar reservation, `DESKTOP_SIDEBAR_WIDTH_PX = 218.0` (`desktop_shell.rs:49`) |
| titlebar strip | `desktop_shell` titlebar reservation, `DESKTOP_TITLEBAR_HEIGHT_PX = 48.0` (`desktop_shell.rs:47`) |
| shell background continuity | `desktop_shell()` root gradient face (`desktop_shell.rs:134`), `DesktopTheme::neutral_dark()` (`modules/ui/src/theme.rs:1125`) |
| composer dock | `NativeComposer` via `cx.new(NativeComposer::new)` inside `ThreadScreen::mount` (`thread_screen.rs:292`) |
| gate precedence opened > loading > failure | `ThreadScreenGate::Open` set post-mount, mirroring `native_application.rs:8052` |

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

1. Root registers the module (`mod parity_visual_proof;`, feature-gated at
   root discretion) and enables `test-support` on the workspace
   `gpui_platform` dependency plus Bazel wiring.
2. Root runs the fixture binary/harness on Windows; it opens two hidden
   windows (`show: false, focus: false`), 1024x720 and 1536x900 logical,
   mounts shell + screen + gate-Open, captures on next frame, and saves
   `parity-proof-{narrow,wide}-{logical}-scale{measured}-{WxH}.png`.
3. Expected physical sizes at the 125% reference scale: narrow 1280x900,
   wide 1920x1125. The fixture asserts `logical * measured scale` from
   `window.scale_factor()` and fails loudly on mismatch instead of claiming.
4. The pure scene manifest (`print_scene_manifest`, six states) runs first
   and needs no renderer.

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
