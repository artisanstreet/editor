# GPUI 0.2.2 capability audit

Phase 6 · UI archaeology and SVG asset foundation · pinned-GPUI capability audit

## 0. Provenance and scope of this document

| Field | Value |
| --- | --- |
| Author role | Phase 6 implementation worker reporting to the Phase 6 lead |
| Repository/worktree | `editor-agent-worktrees/phase-6-gpui`, branch `agent/phase-6-gpui`, base commit `0cfb0a0` |
| Audited crate | `gpui 0.2.2`, exact extracted source at `C:\Users\sander\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\gpui-0.2.2` |
| Pin confirmation | root `Cargo.toml` line 26: `gpui = { version = "=0.2.2", features = ["inspector"] }`; root `Cargo.lock` lines 2413–2416: `name = "gpui"`, `version = "0.2.2"`, `source = "registry+..."`, checksum `979b45cfa6ec723b6f42330915a1b3769b930d02b2d505f9697f8ca602bee707` |
| Legacy references | this worktree's `modules/frontend/src/**` (SvelteKit sources), plus the pinned Bits UI **2.18.1** implementation at `C:\Users\sander\Desktop\artisan-editor\modules\frontend\node_modules\bits-ui` (`package.json` `version: 2.18.1`) |
| Method | every claim below was verified by reading the cited source at the cited location in the pinned extraction. No parity was inferred from symbol names alone; where a behavior matters, the implementing function was read |
| Non-goals | no GPUI primitive, widget, or shader was implemented; no Bits UI/DOM API was cloned; `docs/ui/INVENTORY.md` does not exist yet in this worktree (it is a parallel Phase 6 deliverable), so findings are mapped to the behavior-facet vocabulary PLAN.md defines for it (lines 598–608) |

Line numbers refer to the exact pinned extraction above and are stable for it.

### Classification legend

- **Directly usable** — GPUI provides the capability; Artisan consumes it as-is.
- **Adapter** (usable with a small Artisan adapter) — GPUI provides the mechanism; Artisan adds a thin first-party layer to make it ergonomic or complete.
- **Insufficient** — GPUI has a partial facility that does not satisfy the audited legacy behavior by itself.
- **Absent** — nothing in the pinned crate implements it; Artisan must own it entirely (or not have it).
- **Deferred** — deliberately out of the current port per `docs/PLAN.md` (shader-backed effects, line 30).

---

## 1. The selected initial native workflow and its legacy surfaces

The audited workflow is: **project picker → thread list/create → composer → message/transcript rendering**, plus their overlays. Verified legacy call sites in this worktree:

| Surface | Legacy files (under `modules/frontend/src/routes/components/`) | Wrapper/Bits chain observed |
| --- | --- | --- |
| Project picker | `project-selector.svelte`, `project-folder-picker.svelte`, `project-identity-mark.svelte` | local `dropdown-menu` wrappers over Bits `DropdownMenu`; content opens `side="top" align="start" sideOffset={10}` (`project-selector.svelte` lines 160–166); initial highlight forced onto the selected row via `onOpenAutoFocus={FocusSelectedProject}` (lines 128–132, 164); shared follow-highlight pill (`MakeFollowHighlight`, `DropdownHoverSurface`); `ShaderGlassSurface` backdrop (line 167) is **shader-backed → deferred** |
| Thread list/create | `thread-panel.svelte`, `active-thread-light.svelte`, `new-thread-route.svelte`, `thread-route.svelte`, `workspace-header.svelte`, `sectioned-panel.svelte`, `command-menu.svelte`, `thread-hover-rail.svelte` | `command` (Bits `Command` + `useId`) composing local `dialog`; `context-menu` over Bits `ContextMenu` (`thread-hover-rail.svelte`); plain buttons/badges are first-party wrappers |
| Composer | `thread-composer.svelte`, `composer/{controls,steering-lip,attachment-tray,action-failure}.svelte`, `composer/dom.ts`, `model-selector/*` | `lip-card` (first-party), `Button`, image viewer dialog (`image-viewer.svelte` imports Bits `Dialog` directly), model selector uses `popover`, `tabs`, `select`, `collapsible`, `tooltip`; editor surface is `contenteditable="plaintext-only"` (line 574) with per-character placeholder reveal (lines 559–567) and DOM Range/Selection manipulation in `composer/dom.ts` |
| Transcript/messages | `thread-workspace.svelte`, `conversation-{message,item,prompt,status}.svelte`, shimmer-text wrappers | local `scroll-area` (Bits `ScrollArea`) on `thread-workspace.svelte` line 15; follow-tail/history-prepend policy in `$lib/conversation/scroll-position` and lines 376–430; markdown pipeline lives in lib code, not wrappers |
| Overlays | `forge-connection-overlay.svelte`, model-selector popover, command-menu dialog, tooltips (`composer/controls.svelte`, `model-selector/option-tooltip.svelte`), context menus | Bits internals under `dist/internal/`: `floating-svelte/` (positioning), `body-scroll-lock.svelte.js`, `grace-area.svelte.js`, `safe-polygon.svelte.js`, `roving-focus-group.js`, `dom-typeahead.svelte.js`, `data-typeahead.svelte.js`, `presence-manager.svelte.js`, `state-machine.js`, `focus.js`, `tabbable.js`, `animations-complete.js` |

---

## 2. Capability audit

### 2.1 Entity/view ownership and data flow — **Directly usable**

- `src/app/entity_map.rs`: `Entity<T>` (line 377) and `WeakEntity<T>` (line 654); slot-map-backed entity storage with generational handles (`src/app/entity_map.rs`, `EntityMap`).
- `src/app/context.rs`: `Context::notify` (line 229) marks an entity dirty; `Context::emit<Evt>` (line 723) publishes events; `Context::listener` (lines 252–259) converts `&mut App` callbacks into view-state closures via `WeakEntity`.
- `src/view.rs`: `Render` trait with `fn render(&mut self, &mut Window, &mut Context<Self>) -> impl IntoElement` (lines 356–370); `Entity<V>` implements `Focusable` (`src/window.rs` line 445).
- Ownership/data-flow model documented in-tree: `src/_ownership_and_data_flow.rs`.
- `App::open_window<V: Render>` builds a root view entity per window (`src/app.rs` lines 943–976).

Artisan owns: which entities model project catalog, thread list, composer drafts, transcript state. Nothing here needs adaptation.

### 2.2 Subscriptions and effects — **Directly usable**

- `src/app.rs`: `observe` (780), `subscribe` (865), `observe_global` (1522), `observe_new` (1568), `observe_release` (1589), `observe_release_in` (1610), `on_window_closed` (1806).
- `src/subscription.rs`: `Subscription` (159) with `detach` (175) and Drop-based auto-unsubscribe (197).
- Globals: `src/global.rs` (`Global` trait, `GlobalForWindow`-style access via `App`); global mutation observable through `observe_global`.

This replaces Effect layers/Fibers with explicit subscription lifetimes. Artisan owns wiring protocol events into entity updates.

### 2.3 Async/background execution and UI handoff — **Directly usable**

- `src/executor.rs`: `BackgroundExecutor` (31) spawns `Send` futures (145, `spawn_labeled` 154, `block` 190, scoped 321, `timer` 357); `ForegroundExecutor` (44) spawns non-`Send` futures that resume on the main thread (471); `Task<T>` (58) with `detach` (76) and `detach_and_log_err` (92).
- View-context spawn keeps entity safety: `Context::spawn` hands the task a `WeakEntity<T>` + `AsyncApp` (`src/app/context.rs` lines 237–245); `Context::spawn_in` yields an `AsyncWindowContext` (661).
- `src/app/async_context.rs`: `AsyncApp` (17), `AsyncWindowContext` (266), both with `background_spawn` (105, 431).
- Cross-task handoff channels: `async-channel` is already the plan's transport→GPUI boundary (PLAN.md line 339); GPUI itself imposes no channel choice.

Stale-generation rejection for streaming Markdown (PLAN.md lines 312–314) is Artisan product policy layered on these primitives; GPUI neither helps nor hinders.

### 2.4 Focus handles and traversal — **Directly usable** (trapping is an adapter)

- `src/window.rs`: `FocusHandle` (266) with `tab_index()` (312), `tab_stop()` (323), `downgrade()` (332), `focus()` (340), `is_focused` (345), `contains_focused` (351), `within_focused` (357), `dispatch_action` (367); `WeakFocusHandle` (405). Handles are refcounted against a window-global `FocusMap` (228).
- `Window::focused` (1380), `focus` (1386), `blur` (1397), `disable_focus` (1407), `focus_next` (1413), `focus_prev` (1424).
- Tab order engine: `src/tab_stop.rs` — `TabStopMap` (11) orders handles by `tab_index` groups (`begin_group` 92, `end_group` 98), skips non-stops (`next_inner` 135), wraps at both ends; unit tests at lines 318–610 prove ordering semantics including nested/sibling groups and out-of-order indices.
- Element attachment: `InteractiveElement::track_focus` (`src/elements/div.rs` 616) registers the handle into the dispatch tree during paint (`Window::set_focus_handle`, `src/window.rs` 3342; `key_dispatch.rs` 727).

Gaps: there is **no focus-trap primitive** and no directional (arrow) spatial navigation — Bits' `roving-focus-group.js`/`focus.js` behaviors must be reimplemented by the Artisan framework on top of `focus_next`/`focus_prev`/explicit `focus()` calls (adapter). Focus restoration after overlay close is Artisan policy (capture/restore handles around open/close).

### 2.5 Keyboard actions and events — **Directly usable**

- Action system: `actions!` macro (`src/action.rs` 24), `Action` trait (117), registry-backed dynamic dispatch; bindings registered app-wide via `App::bind_keys` (`src/app.rs` 1677).
- Keymap: multi-keystroke sequences supported (`"cmd-k left"` documented at `src/key_dispatch.rs` 47–51); context matching through `key_context(...)` on interactive elements (`div.rs` 658) building a `KeyContext` stack (`key_dispatch.rs` `DispatchTree.context_stack`, 73); matching via `Keymap::bindings_for_input` (`src/keymap.rs` 142) over `Keystroke`s (`src/platform/keystroke.rs`).
- Dispatch order: innermost-focused listener wins, bubbling outward (`key_dispatch.rs` 39–45); listeners run in two phases (`DispatchPhase::Capture` then `Bubble`, `src/window.rs` 65–90); raw key events also available per element: `on_key_down` (387), `capture_key_down` (403), `on_key_up` (419), `capture_key_up` (432), `on_modifiers_changed` (448, all `div.rs`).
- App-level visibility: `App::observe_keystrokes` fires *after* resolution (`app.rs` 1628); `App::intercept_keystrokes` fires *before* and can stop action dispatch (1654).

Escape-as-dismissal, Enter-as-send, arrow navigation in menus are expressed as actions/bindings — idiomatic and sufficient. No adaptation needed beyond Artisan's own binding tables.

### 2.6 Mouse/pointer events — **Directly usable**

Event types in `src/interactive.rs`: `MouseDownEvent` (93, includes `click_count` and `first_mouse` focusing click at 107), `MouseUpEvent` (120), `MouseClickEvent`/`ClickEvent` (144/164, with `Mouse` vs `Keyboard` variants, `is_right_click` 219, `first_focus` 245), `KeyboardButton::{Enter,Space}` (274), `MouseButton` incl. back/forward navigate buttons (284–296), `MouseMoveEvent` (335, `dragging()` 356), `ScrollWheelEvent` (363) with `ScrollDelta::{Pixels,Lines}` (395) and `pixel_delta(line_height)` (418)/`coalesce` (429), `MouseExitEvent` (470), `TouchPhase` (81).

Handler registration on `Interactivity` (`src/elements/div.rs`): `on_mouse_down` (121, bubble + hover-scoped), `capture_any_mouse_down` (141), `on_any_mouse_down` (157), up variants (173/193/209), `on_mouse_move` (263), `on_scroll_wheel` (312), `on_click` (484, synthesizes ClickEvent incl. keyboard clicks), `on_hover(bool)` (525), drag-and-drop: `on_drag` (499, creates a rendered drag view), `on_drag_move` (282) driven by `cx.active_drag`, drop predicates (473).

Hover styling/state and cursor control (`CursorStyle` enum: Arrow, IBeam, Crosshair, ClosedHand, OpenHand, PointingHand, ResizeLeft/Right…, `src/platform.rs` 1408+) integrate through hitboxes rather than CSS pseudo-states.

Bits' long-hover/grace-area (`grace-area.svelte.js`, `safe-polygon.svelte.js`) has no GPUI equivalent — timer-based hover-intent logic is an Artisan adapter over `on_hover` + `Task` timers.

### 2.7 Capture/bubble dispatch and dismissal feasibility — **Directly usable**

- Two-phase dispatch is real, not nominal: mouse/key listeners receive `(event, DispatchPhase, …)` and run capture-root→target then target→root (`src/window.rs` 3697–3707, 3889–3901, 4007); `cx.stop_propagation()` (`src/app.rs` 1712) halts remaining phases; `window.prevent_default()` (`src/window.rs` 1874) suppresses platform defaults.
- Outside-press dismissal: `on_mouse_down_out` (`div.rs` 226–234) fires during capture when the press lands outside the element's hitbox (`!hitbox.contains(&window.mouse_position())`), plus `on_mouse_up_out` (243). This is exactly the Bits outside-layer dismissal semantic.
- Occlusion policy is explicit. `HitboxBehavior::BlockMouse` (`window.rs` 596, set via `occlude_mouse`, `div.rs` 575) makes both `Hitbox::is_hovered` and `Hitbox::should_handle_scroll` false behind the occluder (`window.rs` 575–596), so a full-window modal scrim blocks background pointer interaction and wheel scrolling. `BlockMouseExceptScroll` (624 / `block_mouse_except_scroll`, 589) suppresses hover/click interaction but deliberately leaves background scroll hitboxes eligible (`window.rs` 598–617); it is suitable only where a nonmodal surface intentionally permits background scrolling.
- DOM-style body scroll-lock (`bits-ui/dist/internal/body-scroll-lock.svelte.js`) has no single GPUI analogue. Artisan must select and test modal scroll containment explicitly: modal layers use a full-window `BlockMouse` scrim (and consume any window-global wheel handler), while nonmodal popovers choose whether background scrolling remains enabled. Scroll locking is therefore component policy, not something the native port can discard.

Dismissal orchestration (Escape action, outside-press ordering among stacked overlays, restore focus) remains Artisan-owned sequencing over these primitives.

### 2.8 Window-level and global listeners — **Directly usable**

- `Window::on_mouse_event<Event: MouseEvent>` (`src/window.rs` 3421–3434): window-global handler invoked for every mouse event with the current dispatch phase — the standard place for drag tracking, safe-polygon logic, and global dismissal.
- `App::observe_keystrokes`/`intercept_keystrokes` (see 2.5) are the app-global key taps.
- Platform-window callbacks (resize, move, activation, appearance, should-close, frame request) listed under 2.16.

### 2.9 Scroll containers and virtualized lists — **Directly usable** (scrollbar chrome is an adapter)

- Plain scrolling: `overflow_scroll`/`overflow_x_scroll`/`overflow_y_scroll` (`div.rs` 1049–1063) set `Overflow::Scroll`; scrolled elements carry internal per-element offset state (`InteractiveElementState.scroll_offset`, consumed at `div.rs` 1601–1613); wheel listeners are registered via `on_scroll_wheel` (312) with the phase-and-hitbox-aware handler shape at 1193.
- `ScrollHandle` (`div.rs` 3068): `offset()` 3083, `max_offset()` 3088, `top_item()` 3093, `bottom_item()` 3112, `scroll_to_item(ix)` 3141, `scroll_to_top_of_item` 3151, `scroll_to_bottom()` 3203, `logical_scroll_top/bottom` 3217/3232; `ScrollAnchor::for_handle` (3022) scrolls an arbitrary descendant into view next frame.
- Virtualization: `uniform_list(id, item_count, render_range)` (`src/elements/uniform_list.rs` 22) renders only the visible slice, with `UniformListScrollHandle` (80), `track_scroll` (675), `with_sizing_behavior` (620), and decorations. Variable-height virtualization: `list(state, render_item)` (`src/elements/list.rs` 24) with `ListState::new(item_count, alignment, overdraw)` (216), `ListState::splice(old_range, count)` (264) and `reset(count)` (244) for items whose height changes, `ListAlignment::{Top, Bottom}` (78–83 — `Bottom` is documented "like a chat log"), `ListScrollEvent { visible_range, count, is_scrolled }` (86), and `with_sizing_behavior` (46).

The transcript's history-prepend compensation, follow-tail arming/disarming, and anchor correction animation (`thread-workspace.svelte` 376–430) are product policy Artisan must own — PLAN.md (297) already assigns them to `frontend`. A visible custom scrollbar (Bits `ScrollArea` renders one) is a small Artisan drawing job over `ScrollHandle` state; GPUI ships no scrollbar widget.

### 2.10 Text layout, styling, measurement — **Directly usable**

- Font service: `TextSystem` (`src/text_system.rs` 54) with `add_fonts` (104, embed font binaries), `resolve_font` (150), metrics helpers (`em_width` 211, `cap_height` 244, ascent/descent 254/260, baseline_offset 265), and `line_wrapper(font, size)` (292) returning `LineWrapperHandle` for greedy wrapping measurement (`src/text_system/line_wrapper.rs`).
- Shaping/layout: `WindowTextSystem::shape_line` (365), `shape_text` (409, multi-run styled layout), `layout_line` (535); `LineLayout`/`WrappedLineLayout` in `src/text_system/line_layout.rs`/`line.rs` with bidirectional hit-testing used below.
- Styling: `TextStyle` (`src/style.rs` 354) — font_family (359), font_features (362), font_fallbacks (365), font_size (368), font_weight (374), font_style (377), background_color (380), underline (383), strikethrough (386); refinement stack via `Window::text_style()` (`window.rs` 1440). Fluent styling methods: `text_color` (styled.rs 396), `font_weight` 404, `italic` 496, `font_family` 616, `line_height` 644, `truncate` 123, `whitespace_nowrap` 65.
- Rich runs: `StyledText` (`src/elements/text.rs` 148) with `with_default_highlights(default_style, (Range<usize>, HighlightStyle))` (173) or delayed `with_highlights` (188) — this is precisely the sink for `pulldown-cmark`+`syntect` ranges per PLAN §ui. `HighlightStyle` (`style.rs` 496) supports color, weight, background (507), underline (510), strikethrough (513), `fade_out` (516); `UnderlineStyle` (788).
- Index↔geometry mapping for interaction: `TextLayout::index_for_position` (text.rs 483), `position_for_index` (517), `line_layout_for_index` (548), plus `bounds` (578), `line_height` (583), `wrapped_text` (606). `InteractiveText` (626) wires `on_click` (667)/`on_hover` (686)/`tooltip` (695) onto text ranges — the natural base for markdown links.

### 2.11 Text selection and text editing — split verdict

- **Selection: Absent.** There is no selection concept anywhere in the text elements or styles (searched `selection` across `src/elements/text.rs`, `src/elements/div.rs`, `src/style.rs` — zero hits). Transcript copy-selection, composer caret/selection, and drag-select must be implemented by Artisan using the index↔position APIs above plus paint-time underline/background highlighting. This matches PLAN's assignment of Markdown selection/copy to first-party code (line 310).
- **IME plumbing: Directly usable.** `InputHandler` trait (`src/platform.rs` 995 — "interface for handling text input from the platform's IME system") with `selected_text_range` (1000), `marked_text_range` (1011), `text_for_range` (1017), `replace_text_in_range` (1029), `replace_and_mark_text_in_range` (1043), `unmark_text` (1054), `bounds_for_range` (1060, positions the IME candidate window), `character_index_for_point` (1070). Installed per frame via `Window::set_input_handler` (`window.rs` 3392–3412) and `ElementInputHandler<V>` (`src/input.rs` 77) binds it to an element's bounds; candidate-window relocation via `update_ime_position` (`platform.rs` 551, `window.rs` 4126–4131).
- **Editable text widget: Absent.** No `TextInput`/`Editor`/textarea primitive exists in the crate. The composer's `contenteditable="plaintext-only"` surface with inline image markers, paste/drop gestures, and range surgery (`composer/dom.ts`) must become a first-party Artisan editor built on: `InputHandler`/IME, key actions, `index_for_position` hit-testing, `StyledText` painting, and `paint_underline`/backgrounds for composition marks. This is the single largest build-vs-provide delta in the audit and is flagged for Phase 7 prototyping (§4).

### 2.12 Clipping, layering, z-order, occlusion — **Directly usable**

- Content masks clip painting: `ContentMask` applied via overflow-hidden styling (`style.overflow_mask` usage at `div.rs` 1674/1825) and threaded through hitboxes (`Hitbox.content_mask`, `window.rs` 533).
- Paint order = scene order, with escape hatches: `deferred(child)` (`src/elements/deferred.rs` 7) postpones layout-completed children to paint after all ancestors, ordered by `priority()` (25/92) — the popover/tooltip stacking tool.
- Occlusion bookkeeping: `bounds_tree.rs` maintains the rectangle tree behind hit-testing; per-frame `mouse_hit_test = next_frame.hit_test(mouse_position)` with `hover_hitbox_count` (`window.rs` 2065, consumed at 500–508).
- Root compositing order is explicit in `draw_roots` (`window.rs` 2013–2086): root tree → sorted deferred draws → prompt → active drag → tooltip, each painted last-wins.

No GPU-side layer isolation exists (no opacity groups/compositing beyond alpha quads); translucency is ordinary RGBA. That suffices for the audited surfaces given shaders are deferred.

### 2.13 Anchored positioning against window bounds — **Directly usable**

`anchored()` (`src/elements/anchored.rs` 27): `anchor(Corner)` (40), `position(Point)` (47), `offset` (54, e.g. Bits' `sideOffset`), `position_mode(Window|Local)` (62, enum at 256), `snap_to_window()` (68) and `snap_to_window_with_margin(edges)` (74). Fit algorithms read from the implementation: `SwitchAnchor` flips the corner horizontally then vertically when the desired bounds leave the viewport (157–182); snapping clamps to viewport edges honoring margins and `client_inset` (184–207). This covers Bits floating-ui's flip/shift behavior for the picker dropdown (`side="top" align="start" sideOffset={10}`) and popovers.

Limits worth recording: collision resolution is whole-bounds corner flipping/snapping — there is no middleware pipeline, no per-axis "keep trigger in view" adjustment beyond snap, and no automatic viewport-relative max-height. Menu max-height-with-scroll is Artisan styling (`max_h` + `overflow_y_scroll`). Built-in tooltip placement has its own simpler flip logic (`prepaint_tooltip`, `window.rs` 2088–2119).

### 2.14 Animation and frame scheduling — **Directly usable** (curves beyond the set are Artisan's)

- Frame loop: `Window::refresh` (1367) requests redraw; `request_animation_frame` (1654) schedules the next frame; `on_next_frame(callback)` (1644) defers one-shot work; platform `on_request_frame` callback (`platform.rs` 490) ties to vsync (`vsync.rs` on Windows).
- Declarative animation element: `Animation { duration, oneshot, easing }` (`src/elements/animation.rs` 15) applied via `.with_animation(id, anim, animator)` (54) or chained `with_animations` (72); drives progress deltas and re-requests frames until done (176). Easings shipped: `linear` (213), `quadratic` (218), `ease_in_out` (223), `ease_out_quint` (233), `bounce(easing)` (238), `pulsating_between(min,max)` (249).
- Timers: `BackgroundExecutor::timer` (executor.rs 357), `smol_timeout` util.

There is **no spring/physics interpolator and no interruptible-transition manager** in 0.2.2 — Bits' presence/timing machinery (`presence-manager.svelte.js`, `animations-complete.js`) translates into Artisan-owned enter/exit state machines that hold an element mounted while its exit animation runs, using the above primitives. Per-character placeholder stagger (`--placeholder-delay`, thread-composer 559–567) maps to either `with_animations` chains or per-character offsets computed in render — an adapter decision for Phase 7.

### 2.15 Image and SVG rendering — **Directly usable** (assets flow through `AssetSource`)

- SVG: `svg()` (`src/elements/svg.rs` 18) paints by asset path tinted with the current `text.color` (paint implementation 90–121 calling `Window::paint_svg`, `window.rs` 3065); rasterized by `SvgRenderer` (`src/svg_renderer.rs` 18) through `resvg`/`usvg` to an alpha mask (57–103), cached per `(path, device size)`, upscaled by `SMOOTH_SVG_SCALE_FACTOR` (9). Sources resolve via `AssetSource::load(path)` (`src/assets.rs` 13–19) installed with `Application::with_assets` (`app.rs` 155) — the typed `assets` crate plugs in exactly here, serving embedded vendored SVG bytes under stable IDs (small adapter: map `AssetId` → registry path).
- Raster images: `img(source)` (`src/elements/img.rs` 198) accepting `ImageSource::{Resource, Render(Arc<RenderImage>), Image, Custom}` (41–50); `RenderImage` is BGRA frame data (`assets.rs` 42–80); style options grayscale/object_fit/loading/fallback (`ImageStyle` 128–133) with a built-in 200 ms loading delay constant (31); painted via `Window::paint_image` (`window.rs` 3129). Animated frames supported (`Frame` vectors).
- Raw drawing beneath both: `canvas(…)` (`src/elements/canvas.rs` 10), `Path` builder (`src/path_builder.rs`, `scene.rs` 676), `Window::paint_quad` (2839), `paint_path` (2860), `paint_underline` (2877), `paint_glyph` (2948); scene primitives `Underline` (scene.rs 472), `MonochromeSprite` (621), `PolychromeSprite` (639).

### 2.16 Platform window and system APIs — **Mixed, Windows-relevant detail**

Trait surface `Platform` (`src/platform.rs` 164–280) and `PlatformWindow` (461–551), Windows backend under `src/platform/windows/`:

| Capability | Status | Evidence |
| --- | --- | --- |
| Window creation/options (bounds, min/max size, titlebar kind, display) | Directly usable | `App::open_window` `app.rs` 943; `WindowOptions`/`WindowBounds{Fixed,Maximized,Fullscreen}` `platform.rs` 1188; `Window::content_size/resize/scale_factor/is_maximized` 461–466 |
| Native file dialogs | Directly usable (Windows implemented) | `prompt_for_paths`/`prompt_for_new_path` return `Receiver`s (`platform.rs` 218–222); Windows impl spawns the dialog off-thread and answers a oneshot (`windows/platform.rs` 442–473); `can_select_mixed_files_and_dirs` returns **false** on Windows (475–478) — folder-picker UX must tolerate files-only or use folders flag |
| Reveal/open in Explorer, recent documents/jump list | Usable | `reveal_path` (480, `open_target_in_explorer`), `open_with_system` (494); `add_recent_document`/`update_jump_list` (`platform.rs` 241–248, `destination_list.rs` on Windows) |
| Clipboard | Directly usable | `write_to_clipboard/read_from_clipboard` (`platform.rs` 264–267, `windows/clipboard.rs`); `ClipboardItem`/`ClipboardEntry::{String,Image}` (1504–1545) |
| Native OS menu bar | Insufficient on Windows | `set_menus` stores menus (`platform.rs` 234) but the Windows backend never creates a Win32 menu (no `SetMenu/CreateMenu/DrawMenuBar` anywhere under `src/platform/windows/` — searched; `windows/platform.rs` 516–522 only caches `OwnedMenu`s). macOS-only fidelity; Artisan must not depend on a visible native menu bar on Windows |
| Window controls/custom titlebar | Directly usable | `WindowControlArea{Drag,Close,Max,Min}` (`window.rs` 480–488) via `window_control_area()` (`div.rs` 581); `minimize/zoom/toggle_fullscreen/set_title/set_background_appearance` (`platform.rs` 484–489); hit-test hook `on_hit_test_window_control` (497); raw `HWND` escape hatch `get_raw_handle` (528) |
| Display/appearance/theme | Directly usable | `displays/primary_display` (`app.rs` 999–1006); `WindowAppearance` via `App::window_appearance` (1029) and `PlatformWindow::appearance` (467) with change callback `on_appearance_changed` (499) — the dark/light switch hook replacing `mode-watcher` |
| Modal prompts | Directly usable | `Window::prompt` (`window.rs` 4141) with native or custom-view prompts; `PromptLevel` (`platform.rs` 1343); testable via `simulate_prompt_answer` |
| Screen capture | Present, unused by workflow | `screen_capture_sources` (`platform.rs` 190–193, `scap_screen_capture.rs`) |
| Credentials | Present | `write_credentials/read_credentials/delete_credentials` (`platform.rs` 269–271) |
| URL opening/schemes | Directly usable | `open_url` (214), `on_open_urls` (215), `register_url_scheme` (216); Windows impl at `windows/platform.rs` 424 |
| Cursor | Directly usable | `set_cursor_style` (259); Windows posts a message to swap HCursors (`windows/platform.rs` 553–564) |

### 2.17 Menus, context menus, popovers, tooltips, dialogs — primitives inventory

Verified absence/presence inside gpui 0.2.2:

- **Tooltips: present as infrastructure.** `StatefulInteractiveElement::tooltip`/`hoverable_tooltip` (`div.rs` 538/555) enqueue a view-builder; the window lays out the latest request at the mouse position, flips/clamps within the viewport (`prepaint_tooltip`, `window.rs` 2088–2119), tracks hovered-tooltip bounds (`TooltipId::is_hovered`, 633; `TooltipBounds` 644), and paints tooltips above everything except prompts and drags (2047–2082). What is missing vs Bits: delay grouping scopes, grace areas, `ignoreNonKeyboardFocus` nuances — Artisan's tooltip component adds timing policy on top (adapter).
- **Popovers/menus/select/command palette: absent as widgets.** No `popover`, `menu`, `select`, or `command` components exist (searched; only incidental `CursorStyle::ContextualMenu` mapping in `wayland.rs` 37). They are built from: `deferred(priority)` for stacking, `anchored()` for placement/collision, `HitboxBehavior::BlockMouse` for light-dismiss modality, `on_mouse_down_out` for outside-press, focus/tab-stop management for roving focus, and actions for typeahead-free keyboard models (typeahead itself is Artisan logic à la Bits' `dom-typeahead.svelte.js`).
- **Dialog/modal: minimal.** Only `ManagedView` + `DismissEvent` (`window.rs` 453–458) as a lifecycle contract ("the lifecycle of the view is handled by another view", with `dismiss_on_focus_lost`-style patterns left to apps) and `Window::prompt` for simple modals. Full dialog/sheet semantics (overlay dimming, focus trap/restore, Escape, scroll containment) are Artisan framework components.
- **Drag overlay: present.** `cx.active_drag` renders the dragged view above all roots at the cursor (`draw_roots` 2055–2060), with `on_drag`/`on_drag_move`/drop predicates (div.rs 282–517).

Classification: the **mechanisms** are directly usable; the **composed overlay widgets** are the core deliverable of Artisan's `modules/ui` crate (Phase 7), designed from the INVENTORY records rather than cloned from Bits.

### 2.18 Accessibility — **Absent**

Exhaustive search of the pinned crate for accessibility facilities returned nothing functional: no `accesskit`, `UIAutomation`, `NSAccessibility`, `AxNode`, role/name/label attributes, or accessible-tree emission anywhere in `src/` (hits for "accessible" were prose comments only, e.g. `app.rs` 1805). The crates.io 0.2.2 release predates Zed's later accessibility work.

Consequences for the port: DOM-era aria intent recorded in INVENTORY (`aria-label` on triggers, roles, expanded/selected states) cannot map to any platform accessibility API today. Artisan should (a) keep semantic labels/state in first-party component state so an accessibility layer can attach later, and (b) treat screen-reader support as an explicitly tracked limitation, not something to fake. Flagged for Phase 7 as an unknown requiring its own decision (§4).

### 2.19 Reduced motion and theme hooks — **Absent / hook present**

- **Reduced-motion: absent.** No `prefers-reduced-motion` equivalent, no OS setting query, nothing in `Platform`/`PlatformWindow` (searched `reduced.?motion|prefers` across `src/` — zero hits). Artisan owns a global motion policy (theme global consulted by every animation call site; PLAN line 607 requires the reduced-motion rule anyway). Windows "show animations" detection would require first-party Win32 query via the existing `get_raw_handle`/platform-impl seam if desired — Phase 7 decision.
- **Theme/light-dark: OS hook directly usable, tokens Artisan-owned.** `WindowAppearance` polling/callback (2.16) supplies system light/dark; color tokens, typography scale, spacing, radii, and interaction states are pure first-party values (GPUI has `colors.rs`/`Hsla` utilities only). Font loading via `TextSystem::add_fonts` (104) backs the vendored font strategy.

### 2.20 Testing facilities — **Usable, gated behind a non-default feature**

- Deterministic harness: `#[gpui::test]` macro (re-exported from `gpui_macros`, `src/gpui.rs` 81) driving seeded iterations with retries (`src/test.rs` `run_test` 41–131, seed env overrides 85–130); `TestDispatcher` gives deterministic executor ordering.
- `TestAppContext` (`src/app/test_context.rs`): `add_window_view` (256) / `add_empty_window` (235) produce a real window + `VisualTestContext`; simulation APIs cover `simulate_keystrokes` (419, 715), `simulate_input` (435), `dispatch_keystroke` (444), `simulate_mouse_move/down/up/click` (726–771), `simulate_window_resize` (330), `simulate_new_path_selection` (301), `simulate_prompt_answer` (310), clipboard (290–298); observation streams (`src/test.rs` `observe` 151).
- **Gate:** these symbols compile only under `#[cfg(any(test, feature = "test-support"))]` (e.g. `src/gpui.rs` 42–43, `app.rs` 30/52, `test_context.rs` 535), and upstream `[features]` defines `test-support` (`gpui-0.2.2/Cargo.toml.orig` 19–31). The repository pin currently enables only `inspector` (root `Cargo.toml` 26). External `tests/ui/` targets therefore require adding the `test-support` feature to the gpui dependency — a root-manifest change that must go through VP approval; flagged in §4.

---

## 3. Mapping to the INVENTORY behavior vocabulary

How each behavior facet PLAN.md expects in `docs/ui/INVENTORY.md` (lines 599–608) lands on GPUI 0.2.2:

| Inventory facet | GPUI substrate | Attribution of remaining behavior |
| --- | --- | --- |
| Call sites, composition, defaults, variants, visual states | styled element trees; variant enums are plain Rust (no `tailwind-variants` runtime) | Artisan wrapper/component code |
| Controlled/uncontrolled state, transitions, callback ordering | entity state + `Context::notify`; listener registration order = dispatch order within an element (`Interactivity` vec push order, `div.rs`) | Artisan owns state machines Bits kept internally (`state-machine.js`, `shared-state.svelte.js`) |
| Keyboard commands, roving focus, typeahead, activation | actions/bindings (2.5), `FocusHandle`/tab stops (2.4) | roving-index and typeahead logic ported from Bits semantics into `ui` components |
| Focus entry, trapping, restoration; disabled/invalid/read-only/loading states | `focus()`, `focus_next/prev`, `track_focus`; disabled = Artisan predicate skipping listeners/styling | trap/restore orchestration Artisan-owned (no primitive); state styling Artisan-owned |
| Pointer, hover, grace area, outside-press, Escape, dismissal | mouse events (2.6), `on_mouse_down_out`, `stop_propagation` (2.7), actions for Escape | grace-area/safe-polygon timers ported into `ui`; dismissal sequencing Artisan-owned |
| Overlay ordering, modality, portals, scroll locking, anchor geometry, collision, transform origins | `deferred(priority)`, modal `BlockMouse` vs policy-specific nonmodal `BlockMouseExceptScroll`, `anchored()` flip/snap, explicit root order prompt>drag>tooltip (2.12–2.13, 2.17) | "portal" concept disappears (deferred draws are in-window); modal scroll containment and global-wheel consumption remain Artisan-owned; transform-origin polish (Bits `--bits-*-content-transform-origin` variables) is derived from the chosen anchor corner and verified visually |
| Presence, open/close timing, animation, reduced motion | `AnimationElement`, frame scheduling (2.14); reduced-motion absent (2.19) | exit-presence managers + motion policy Artisan-owned |
| Accessibility intent, labels, announcements | **nothing** (2.18) | retained as first-class component metadata; platform announcement impossible today — recorded limitation |
| Behavior attribution (Artisan / wrapper / Bits) | n/a | carried per-row by INVENTORY; this doc fixes the GPUI column |

---

## 4. Proposed native seams, invariants, and Phase 7 risks

### Seams (idiomatic, no Bits/DOM cloning)

1. **Asset seam:** `Application::with_assets(Box<dyn AssetSource>)` backed by the `modules/assets` typed API; `svg()` paths become `AssetId`-derived constants. Icon tint always flows through `text_color`.
2. **Overlay seam:** one first-party layer pattern — `deferred(...).with_priority(n)` + `anchored().anchor(corner).offset(...)` + explicit modality policy + `on_mouse_down_out(dismiss)` + focus entry/save/restore — parameterized per INVENTORY contract instead of five ad-hoc copies. Modal layers use a full-window `BlockMouse` scrim and consume background wheel events; a nonmodal popover may opt into `BlockMouseExceptScroll` only when its contract allows background scrolling.
3. **Action seam:** every dismissible overlay listens for a `Dismiss`-style action bound per keymap context; Escape never hand-rolls key listeners.
4. **Editor seam:** composer editor = entity owning text buffer + selection + composition state, exposing `InputHandler` via `ElementInputHandler`, painting through `StyledText`; image attachments as owned inline objects rather than DOM markers.
5. **Transcript seam:** `list(ListState, …)` with `ListAlignment::Bottom` for the live tail plus Artisan prepend-compensation in `frontend` (product policy per PLAN 297), never baked into a generic scroller.
6. **Motion seam:** all durations/easings route through theme motion tokens checked against the reduced-motion policy before any `with_animation` call.

### Invariants worth enforcing in Phase 7 tests

- At most one `FocusHandle` is focused per window; having none is valid. A modal must establish its documented initial focus on entry, contain traversal while open, and restore the previously focused handle on dismiss when that handle is still valid.
- Deferred-draw priorities form a total order documented in `ui` (scrim < popover < tooltip < prompt < drag mirrors `draw_roots`).
- Every outside-press dismissal is capture-phase (`on_mouse_down_out`) so bubble handlers of the newly pressed element still run.
- Streaming Markdown repaints only the changed prefix's entity; stale generations dropped (ties 2.3 to PLAN 312–314).

### Risks / unknowns Phase 7 must prototype (not resolved here)

1. **Composer editor feasibility** (largest): IME round-trip on Windows via `ElementInputHandler`, plaintext-only semantics, inline image markers, caret painting — prototype before committing the `ui` API.
2. **`test-support` feature enablement**: required for `tests/ui/`; root-manifest change needs VP approval and Crate Universe repin.
3. **Transcript performance**: `list` overdraw/`splice` behavior under high-frequency stream updates and prepend compensation; measure before trusting.
4. **Anchored collision edge cases**: tall menus near viewport bottom (flip may be unavailable → snap leaves the trigger covered); compare against Bits floating-ui outcomes on the picker/model-selector geometries.
5. **Exit animations**: no presence manager; prototype a minimal keep-mounted-until-exit-finishes helper and confirm no flicker with deferred priorities.
6. **Windows native menu bar inertness**: decide whether the app needs an in-app menu/command surface (likely, given command-menu legacy).
7. **Accessibility strategy**: confirm no accessibility path exists in-pin (verified) and record whether shipping without it is acceptable for v1 or whether a newer gpui/vendor patch becomes a separate approved decision.
8. **Tooltip timing policy**: build delay/hide/grace semantics over the built-in tooltip plumbing and verify hover-through (`hoverable_tooltip`) behaves like Bits' safe-polygon.

---

## 5. Dependency policy note

First-party `anyhow` remains forbidden (PLAN Error policy, lines 357–360). GPUI itself depends on `anyhow` transitively and even re-exports it (`gpui-0.2.2/src/gpui.rs` 54–59 `pub mod private { pub use anyhow; … }`, line 69 `pub use anyhow::Result`; in-crate usage such as `svg_renderer.rs` 61 `anyhow::ensure!`). Because gpui is third-party, its internal choice requires no fork, patch, or exception, and first-party code must simply not import or re-export it.

---

## 6. Validation record

- Pin: root `Cargo.toml` line 26 and `Cargo.lock` lines 2413–2416 match the audited extraction path and version 0.2.2.
- Every `src/...` citation above was opened and read in `C:\Users\sander\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\gpui-0.2.2` during this session; negative claims (accessibility, reduced motion, selection, popover/menu widgets, Windows menu bar, `SetMenu*` absence, `test-support` gating) were established by targeted searches whose zero-hit results are noted inline.
- Legacy citations reference files present in this worktree at commit `0cfb0a0` (`modules/frontend/src/routes/components/**`); Bits UI citations reference the pinned 2.18.1 install at `C:\Users\sander\Desktop\artisan-editor\modules\frontend\node_modules\bits-ui`.
- Checks run: working tree clean before edits (`git status`), audit limited to creating this document, single commit produced afterward containing only this file. Commit SHA reported to the controller; no push, PR, stack manipulation, or merge performed.
