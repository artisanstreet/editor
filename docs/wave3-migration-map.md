# Wave 3 migration map — gpui 0.2.2 → gpui-ce (explorer output, 2026-09-04)

> Canonical checkout: master @ `287da9b8`. Old API: gpui 0.2.2 (registry source).
> New API: `artisanstreet/gpui-ce` main (vendor pin `0b84630e`, branch `artisan/backdrop-blur` ≈ main).
> Vendor lane: `wt-wave3-vendor` @ `agent/wave3-vendor-vp1` — uncommitted diff = 72 files, 1890+/1660−. `DONE-BY-VENDOR` refers to that diff; compiles only after §4-P0.

## 1. Inventory

### Build wiring (area e)

| File | Today | Status |
|---|---|---|
| root `Cargo.toml:34` | `gpui = { version="=0.2.2", features=["inspector","test-support"] }` | DONE-BY-VENDOR: `gpui={package="gpui-ce",path="vendor/gpui-ce/crates/gpui",version="=0.2.2",features=[...]}` + `gpui_platform={package="gpui_ce_platform",path="vendor/gpui-ce/crates/gpui_platform"}` + `exclude=["vendor/gpui-ce"]`. **Check the `=0.2.2` version constraint on the path dep.** |
| `modules/frontend/Cargo.toml:20` | `gpui.workspace=true` | DONE-BY-VENDOR: + `gpui_platform.workspace=true` |
| `modules/ui/Cargo.toml:12` | `gpui.workspace=true` | unchanged (correct) |
| `MODULE.bazel:30-42` | `crate.from_cargo(...)` + `crate.annotation(crate="gpui",version="0.2.2",patches=["//third_party:gpui-blur.patch"])` | DONE-BY-VENDOR (comment-only): annotation removed; **Bazel unverified — no bazel binary on host. OPEN (P9).** |
| `BUILD.bazel:26-344` | clippy/rustfmt targets via crate_universe | OPEN: repin `Cargo.Bazel.lock` + label remap (`@crates//:gpui` → `gpui-ce`) verification |
| `Cargo.lock` / `Cargo.Bazel.lock` | gpui 0.2.2 + blade chain | DONE-BY-VENDOR (Cargo.lock): gpui-ce, gpui_ce_platform, gpui_ce_collections/macros/refineable/scheduler/shared_string, wgpu chain replaces blade. `Cargo.Bazel.lock` OPEN. |
| `.gitmodules` + `vendor/gpui-ce @0b84630e` | new | DONE-BY-VENDOR: submodule `artisanstreet/gpui-ce` branch main |
| `third_party:gpui-blur.patch` | patched 0.2.2 `scene.rs` `BlurRect` | DONE-BY-VENDOR (reference deleted): CE has no `BlurRect` there; blur re-port = blur lane onto `vendor/gpui-ce` (`style.rs:444 Filter::Blur`, `Styled::backdrop_filter`). OPEN. |

Only `modules/ui` and `modules/frontend` depend on gpui directly. All other workspace members have zero gpui deps — no migration.

### (a) modules/ui components (44 files)

DONE-BY-VENDOR: `accordion.rs` (NamedChild Box→Arc :675,686), `alert_dialog.rs` (BoxShadow inset :251, gpui::white :273,281, NamedChild Arc :547, ColorExt), `asset_seam.rs` (**WITH P0 BUG** :171), `button.rs` (:472 inset:false), `card.rs` (:113 inset:false), `command.rs` (:523-524,1076,1251), `context_menu.rs` (Anchor :1433, NamedChild Arc :1096-1105, focus(h,cx) ×5, inset:false :811), `dialog.rs` (:166-173 cx:&mut App, gpui::black :348, inset:false :383, NamedChild Arc :587), `dropdown_menu.rs` (focus(h,cx) ×7 :1062-1140), `fade_arc.rs` (ColorExt, NamedChild Arc :688), `input.rs` (:521), `input_group.rs` (:841), `link_preview.rs` (Corner→Anchor :14,1442), `native_select.rs` (:737), `popover.rs` (NamedChild Arc :781), `progress.rs` (Hsla alpha :146-148), `scroll_area.rs` (:242), `select.rs` (NamedChild ×4, focus ×4), `sheet.rs` (:220-227,446,707), `shimmer_text.rs` (test only), `slider.rs` (:513,583), `switch.rs` (:304), `tabs.rs` (:463), `textarea.rs` (:511), `theme.rs` (hsla() :128,138, inset:false :635-640, InsetShadowLayer doc :647-653), `toggle.rs` (:476), `toggle_group.rs` (:797,812).

OPEN-VERIFY (src expected clean): `alert.rs`, `avatar.rs`, `badge.rs`, `collapsible.rs`, `icon.rs`, `input_state.rs`, `lib.rs`, `lip_card.rs`, `list_row.rs`, `markdown.rs`/`markdown_renderer.rs`, `separator.rs`, `skeleton.rs`, `tooltip.rs`.

OPEN-DECISION: `fonts.rs` (`register_bundled_fonts` :27-33 → `app.text_system().add_fonts` — verify CE DirectWrite path); `motion.rs` (CE springs exist, intentional non-adoption).

### (b) modules/frontend (162 files; 37 import gpui)

DONE-BY-VENDOR (12): `conversation_surface.rs`, `editor_route_screen.rs`, `image_viewer.rs`, `native_application.rs`, `native_command_menu.rs`, `native_composer.rs`, `native_new_thread_surface.rs`, `native_project_menu.rs`, `native_thread_picker.rs`, `project_picker.rs`, `proof.rs`, `thread_screen.rs`.

OPEN-VERIFY / STABLE (25): `attention.rs`, `composer.rs`, `composer_send_readiness.rs`, `conversation_delivery_machine.rs`, `conversation_host.rs`, `conversation_projection.rs`, `conversation_scene.rs`, `conversation_scroll_position.rs`, `conversation_steering_machine.rs`, `conversation_turn_machine.rs`, `conversation_view_machine.rs`, `engine_settings.rs`, `lib.rs`, `markdown_fence_policy.rs`, `native_hover_rail_card.rs`, `native_route.rs`, `native_settings.rs`, `native_transport_service.rs`, `object_url_boundary.rs`, `onboarding_screen.rs`, `project_identity_policy.rs`, `shell.rs`, `transcript.rs` + ~125 pure-logic policy files (no gpui import, no migration).

### (d) Test harnesses

- Harness core `tests/ui/gpui_harness.rs` — DONE-BY-VENDOR (`window.focus(&f,app)` :100,129).
- `tests/ui/tinted_svg.rs` — DONE-BY-VENDOR **WITH P0 BUG** (:120).
- `tests/ui/*.rs` 44 files: vendor touched 25; 19 untouched → broken opens (alert :162, badge :149, shimmer_text :80, theme_tokens :35,93,316,335,457,496,501,515,534, context_menu :158,160, alert_dialog :213, dialog :187, sheet :285, scroll_area :203,277,278) vs truly clean (accordion, avatar, collapsible, dropdown_menu, icon, input_state, link_preview, lip_card, list_row, markdown_seam, motion_policy, separator, skeleton, tooltip).
- `tests/frontend/*.rs` 146 files, 7 import gpui: `conversation_surface.rs` (partial DONE), `project_picker_native.rs` (DONE), `native_thread_picker.rs` (DONE); `conversation_host.rs`, `native_shell.rs`, `image_viewer.rs`, `new_thread_surface.rs` clean. ~139 pure-policy, no migration.

## 2. API usage census (what we call)

- **Styled**: `.bg()` (Hsla + linear_gradient), `.border-*`, `.rounded()`, `.shadow(vec![BoxShadow])` (ring-as-spread pattern), `.size/.w/.h/.size_full`, `.flex/.flex_col/.gap/.py/.px`, `.flex_grow()` (2 sites), `.flex_shrink_0`, `.text_size/.font_family("Artisan Neo")/.font_weight/.text_color`, `.overflow_hidden`, `.opacity()`. ~540 Styled chains in modules/ui.
- **Entity/Context/App**: Entity<V>, Context<Self>, App, AppContext, Render/RenderOnce, Subscription, Task, SharedString, ElementId (incl. NamedChild), FocusHandle, Window. #[gpui::test], TestAppContext, VisualTestContext.
- **Window/titlebar**: `Application::new()` → `gpui_platform::application()`; WindowOptions/TitlebarOptions unchanged.
- **Actions**: `actions!`, KeyBinding::new, key_context/on_action/track_focus, bind_keys.
- **Text/fonts**: `.font_family`, FontWeight, register_bundled_fonts → text_system().add_fonts, TextStyleRefinement via Styled::text_style.
- **Images/assets**: AssetSource (CatalogAssetSource), with_assets, svg().path(), img(ImageSource::Resource), ObjectFit.
- **Masks**: overflow_hidden (rectangular ContentMask) — no API change.
- **Animations**: MotionPolicy → MotionPlan → Animation::new; AnimationExt, canvas, deferred, anchored().snap_to_window(). CE springs/ transitions unused (intentional).
- **Test caps used**: add_window_view, update, simulate_keystrokes/click/event, debug_bounds, run_until_parked, VisualTestContext, FocusHandle::is_focused, ScrollHandle::{max_offset,bounds,set_offset}, debug_selector.

## 3. Drift classes (0.2.2 → CE main)

| ID | Drift | Fix | Status |
|---|---|---|---|
| D1 | `FocusHandle::focus(&self, window)` → `(window, cx)`; `Window::focus(h)` → `(h, cx)`; `focus_next/prev(cx)`; on_action closures gain cx | mechanical | DONE-BY-VENDOR (src+tests) |
| D2 | `Application::new()` removed → `gpui_platform::application()` + dep `gpui_ce_platform` | mechanical + build | DONE-BY-VENDOR |
| D3 | `on_window_closed(fn(&mut App))` → `(fn(&mut App, WindowId))` | mechanical | DONE-BY-VENDOR |
| D4 | `ElementId::NamedChild(Box, SharedString)` → `(Arc, SharedString)` | mechanical | DONE-BY-VENDOR |
| D5 | `Hsla{h,s,l,a}` → `palette::Hsla{color,alpha}` private fields; `hsla()` stays; free `black()/white()`; `ColorExt::opacity/blend/fade_out` | mechanical | PARTIAL — src DONE; **tests OPEN** (see P8) |
| D6 | `BoxShadow` + `inset:bool` + builders | mechanical (`inset:false`) | DONE-BY-VENDOR (src); InsetShadowLayer deferred |
| D7 | `geometry::Corner` → `Anchor` for `anchored().anchor()` | mechanical | DONE-BY-VENDOR |
| D8 | `Styled::text_style()` → `&mut TextStyleRefinement`; `Is`Empty semantics | mechanical **WITH VENDOR BUG (P0)** | OPEN |
| D9 | `ScrollHandle::max_offset() Size` → `Point`; `.width/.height` → `.x/.y` | mechanical | PARTIAL — src DONE; tests OPEN |
| D10 | `flex_grow()` → `flex_grow(f32)` + `flex_grow_1()` | mechanical | DONE-BY-VENDOR |
| D11 | `linear_gradient` default Srgb → hardcoded **Oklab** | needs-decision (semantic; fidelity gain — visual sign-off via typography_gradient + gradient_avatar) | no code fix |
| D12 | `KeyDownEvent` + `prefer_character_input` | mechanical | DONE-BY-VENDOR (slider) |
| D13 | `Hsla::black()/white()` assoc fns → free fns + ColorExt | mechanical | src DONE; tests OPEN |
| D14 | `paint.a = alpha` → `.alpha` | mechanical | DONE-BY-VENDOR |
| D15 | `Hsla::from(Rgba)` → `rgb_to_hsla(rgb(...))` | mechanical | DONE-BY-VENDOR |
| D16 | New capabilities NOT adopted (deliberate): Filter::Blur/backdrop_filter vs BackdropBlurIntent; BoxShadow::inset(true) vs InsetShadowLayer; CE springs vs MotionCurve::CheckBob | needs-decision, later packets | OPEN |
| D17 | Features: blade-* → wgpu-surfaces/gpui_wgpu; font-kit default; inspector/test-support preserved | feature wiring | vendor enabled wgpu chain; Bazel OPEN |

## 4. Packet cards (disjoint ownership)

### P0 — Vendor-bug fixups (BLOCKS EVERYTHING) — S
- `modules/ui/src/asset_seam.rs:171` + `tests/ui/tinted_svg.rs:120`: `text_style().is_some()` invalid → `!text_style().is_empty()` (+ trait import) preserving unwind semantics (:179-182).
- Checks: tinted_svg + asset_seam tests.

### P1 — UI foundation — S
- P0 + verify fonts.rs add_fonts on CE (Windows DirectWrite) + gradient snapshot sign-off (Oklab).

### P2 — UI primitives A (verify-only + 3 test fixes) — S
- src clean: alert, avatar, badge, collapsible, icon, input_state, lib, lip_card, list_row, markdown*, separator, shimmer_text, skeleton, tooltip (+ accordion, card DONE).
- tests: alert.rs:162, badge.rs:149, shimmer_text.rs:80 (`.a`→`.alpha`).

### P3 — UI forms — M
- DONE-BY-VENDOR src+tests; verify-only (button, input, input_group, native_select, scroll_area, select, slider, switch, tabs, textarea, toggle, toggle_group + their tests).

### P4 — UI overlays — M
- src DONE (alert_dialog, dialog, sheet, popover, context_menu, dropdown_menu, command, link_preview, fade_arc, progress, scroll_area).
- tests OPEN: alert_dialog.rs:213, dialog.rs:187, sheet.rs:285 (`Hsla::black()`→`gpui::black()`); context_menu.rs:158,160 (`.a`→`.alpha`); scroll_area.rs:203,277,278 (`size()`→`point()` for max_offset).

### P5 — Frontend app shell — M
- DONE-BY-VENDOR (native_application.rs 43 sites, proof.rs, shell.rs, native_route.rs, native_settings.rs, lib.rs, main.rs). Needs runtime proof: app opens, quit bindings, window smoke.

### P6 — Frontend pickers/composer/surfaces — M
- src DONE (conversation_surface, editor_route_screen, image_viewer, native_command_menu, native_composer, native_new_thread_surface, native_project_menu, native_thread_picker, project_picker, thread_screen + stable type-only files).
- OPEN: verify `tests/frontend/conversation_surface.rs` no Size-vs-Point asserts remain (:646,702,1309,1332).

### P7 — Frontend pure policies — S
- ~125 files no-op + 25 stable; prove via `cargo check -p artisan-frontend` + tests/frontend batch.

### P8 — Test-line batch — S/M
- One-line mechanical fixes: tests/ui/alert.rs:162, badge.rs:149, shimmer_text.rs:80, theme_tokens.rs (9 lines), context_menu.rs:158,160, alert_dialog.rs:213, dialog.rs:187, sheet.rs:285, scroll_area.rs:203,277,278. Patterns: `.a`→`.alpha`, `Hsla::black()`→`gpui::black()`, `size()`→`point()` for max_offset.

### P9 — Build/vendor/Bazel — L
- Repin Cargo.Bazel.lock on a Bazel host; label remap `@crates//:gpui` → `gpui-ce`/`gpui_ce_platform`; blur-patch decision; feature propagation (wgpu-surfaces, font-kit, inspector, test-support). **No bazel binary on this host — cannot be proven from cargo alone.**

## 5. Risks / unknowns (semantic)

1. **D11 gradients**: CE hardcodes Oklab; legacy CSS declares no interpolation method → oklab default per CSS Color 4. Expected fidelity GAIN but needs visual sign-off (typography_gradient, gradient_avatar).
2. **Font registration**: verify CE Windows DirectWrite path honors vendored fonts before any window paints; silent fallback otherwise.
3. **Asset source**: CatalogAssetSource + with_assets signatures unchanged; watch path-key vs URI misclassification + resvg/usvg delta.
4. **Window/titlebar**: construction path changed (with_platform); activation, bounds, quit-mode, on_window_closed timing need runtime smoke.
5. **Test harness**: preserved, but audit synthetic-event constructors (KeyDownEvent grew).
6. **Scroll semantics (D9)**: max_offset Size→Point — confirm coordinate-space equivalence in conversation_surface clamps.
7. **Text-style emptiness (D8/P0)**: fix must preserve scoped-tint precedence + unwind exactly.
8. **Shadow inset (D6)**: InsetShadowLayer deliberately unconverted — later packet must not reinterpret `--shadow-inset` as outer.
9. **Backdrop blur (D16)**: BackdropBlurIntent still intent-only; blur parity claimed only after consume-blur packet.
10. **Motion springs (D16)**: CE springs intentionally unused; adoption is a design decision with snapshot impact.
11. **Renderer/features**: blade→wgpu, font-kit, profiler — native gate (--jobs=1, <85% CPU, ≥6GiB) must prove Windows render before stack merges.

## 6. Bazel wiring

Today: `crate.from_cargo` + `crate.annotation(crate="gpui",version="0.2.2",patches=["//third_party:gpui-blur.patch"])` → `@crates//:gpui`.
After: path dep `vendor/gpui-ce/crates/gpui` (package `gpui-ce`) + `gpui_ce_platform`; old annotation selector matches nothing; vendor removed it with comment.
P9 remains: (1) repin Cargo.Bazel.lock; (2) label remap `@crates//:gpui` → sanitized `gpui-ce` across BUILD files; (3) blur-patch re-port decision (or upstream to artisanstreet/gpui-ce); (4) verify feature propagation on Windows. No bazel binary on this host — cargo-green ≠ Bazel-green; do not mark P9 done from cargo.
