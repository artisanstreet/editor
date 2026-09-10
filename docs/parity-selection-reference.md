# Selectable text reference (parity-selection)

Worker: `selectable_text.rs` + this note. Shared native selectable transcript
text for the visual-parity checklist (text selection).

## Frozen public API (`artisan_ui::selectable_text`)

No Entity, no transcript/data authority, no editor/IME. One custom `Element`
over one `StyledText`, built with the vendor-proven `InteractiveText`
mechanics (`TextLayout::index_for_position` hit-testing during paint,
`window.on_mouse_event` / `window.on_key_event` registration,
`ClipboardItem::new_string` + `App::write_to_clipboard`).

```rust
// Pure helpers (no window needed)
clamp_to_char_boundary(text, index) -> usize
normalize_selection(anchor, head, text) -> Option<Range<usize>>
is_copy_keystroke(key, &Modifiers) -> bool        // ctrl/cmd-c, no shift/alt
is_select_all_keystroke(key, &Modifiers) -> bool  // ctrl/cmd-a, no shift/alt
merge_selection_highlight(text, base, selection, &wash) -> Vec<(Range, HighlightStyle)>
selection_style_for_theme(ArtisanTheme) -> HighlightStyle  // ::selection tokens

// Controlled handle: caller owns one per text element, passes it every render
SelectableTextState::new()
validate_for_text(&str)   // fingerprint change (any content change) clears
selection_range()         // live anchor/head range while dragging, else retained
has_selection() / suppresses_click() / is_dragging()
selected_text(&str) -> &str / copy_text(&str) -> Option<String>
begin_drag(i) / update_drag(i) -> bool / end_drag(&str) -> bool
down_index() / clear_press() / clear_selection() / select_all()

// Element: per-render value with a stable caller ElementId (GPUI elements are
// immediate-mode values; StyledText/Input work the same way).
SelectableText::new(id, text, &state, theme, base_highlights)  // controlled
SelectableText::retained(id, text, theme, base_highlights)     // default: framework element state
    .focus(FocusHandle)   // controlled only; press-to-focus gates ctrl/cmd-c and ctrl/cmd-a
    .links(ranges, on_link) // fires only if press+release in same range AND no drag selection
    .text() / .selection() / .shows_selection()
SelectableLinkHandler = Rc<dyn Fn(usize, &mut Window, &mut App)>
```

Retained mode persists selection, drag latch, text fingerprint, and focus in
`Window::with_element_state` under the element id — readable during
`request_layout`/`prepaint`/`paint`, released when the element stops
painting. The retained focus handle is attached with
`Window::set_focus_handle` in `prepaint`, so it participates in the key
dispatch tree (no tab stop is claimed). Without a view-backed identity the
element degrades to ephemeral per-frame state instead of panicking.

## Guarantees

- Style preservation: the wash overlays only foreground/background onto
  intersecting caller ranges; weight, font style, underline, and sibling
  ranges survive, so glyph metrics never change while selecting.
- Fingerprint validation: length plus a 64-bit content hash; accidental
  collision ~2^-64 per comparison, adversarial collisions out of threat
  model. Same-length replacements clear the selection.
- Live drag: `selection_range` reports the normalized anchor/head range
  while dragging, so the wash repaints on every pointer move before release
  and copying works pre-release. A press without movement still clears.
- Link safety: a drag that yields a selection consumes its release
  (`stop_propagation`), and `suppresses_click` covers the live range, so
  consumers skip activation deterministically.

## Consumer recipes

- User body (plain): `retained(id, body, theme, Vec::new())`.
- Markdown/code: `retained(id, source, theme, syntax_ranges)` — merge once
  here, because `StyledText::with_highlights` replaces ranges.
- Links: `.links(ranges, handler)` plus `suppresses_click()` in any
  overlapping handler.
- Typography/wrap: inherited from the parent `text_style` at layout time;
  the element adds no font, size, or wrap of its own.

## Root wiring (not owned by this packet)

1. `modules/ui/src/lib.rs`: add `pub mod selectable_text;`.
2. `modules/ui/Cargo.toml`: add `[[test]] name = "selectable_text",
   path = "../../tests/ui/selectable_text.rs"`.
3. Followup integration packet (separate session): route transcript user
   bodies and `MarkdownRenderer` blocks through `SelectableText::retained`.

## Verification status

- Source-only packet: no builds or native gates per contract (root owns all
  gates). New drag/keyboard/element-state paths mirror vendor
  `InteractiveText` and in-tree `input.rs` / `command.rs` /
  `conversation_host.rs` call shapes; phase use (`with_element_state` and
  `current_view` in layout/prepaint/paint, `set_focus_handle` in prepaint)
  follows the pinned `DrawPhase` guards.
- `tests/ui/selectable_text.rs`: 11 pure tests (clamp, normalize, drag
  lifecycle, unicode, stale-text and same-length invalidation, overlay
  merge with nested styles, live drag range, keystrokes) + 10 GPUI tests
  (controlled mount/select-all/copy/unfocused gate; retained live drag
  copy before release, release-then-copy, streaming reset, link fire,
  link-drag suppression, click-focus select-all). No-op-free.
