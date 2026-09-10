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
merge_selection_highlight(text, base, selection, &style) -> Vec<(Range, HighlightStyle)>
selection_style_for_theme(ArtisanTheme) -> HighlightStyle  // ::selection tokens

// Controlled handle: caller owns one per text element, passes it every render
SelectableTextState::new()
validate_for_text(&str)   // clears selection/drag when the text changed
selection_range() / has_selection() / suppresses_click() / is_dragging()
selected_text(&str) -> &str / copy_text(&str) -> Option<String>
begin_drag(i) / update_drag(i) -> bool / end_drag(&str) -> bool
down_index() / clear_press() / clear_selection() / select_all()

// Element: per-render value with a stable caller ElementId (GPUI elements are
// immediate-mode values; StyledText/Input work the same way). Nothing persists
// in a parallel store; the state handle carries the selection across frames.
SelectableText::new(id, text, &state, theme, base_highlights)
    .focus(FocusHandle)        // press-to-focus; gates ctrl/cmd-c and ctrl/cmd-a
    .links(ranges, on_link)    // fires only if press+release share a range AND no drag selection
    .text() / .selection() / .shows_selection()
SelectableLinkHandler = Rc<dyn Fn(usize, &mut Window, &mut App)>
```

## Consumer recipes

- User body (plain): `new(id, body, &state, theme, Vec::new())`.
- Markdown/code: `new(id, source, &state, theme, syntax_ranges)` — merge once
  here, because `StyledText::with_highlights` replaces ranges.
- Links: `.links(ranges, handler)` plus `state.suppresses_click()` in any
  overlapping handler. A drag that yields a selection consumes its release
  (`stop_propagation`); click (no drag) clears the selection and may
  activate a link when press and release share its range.
- Typography/wrap: inherited from the parent `text_style` at layout time;
  the element adds no font, size, or wrap of its own.
- Focus is pointer-driven (no tab stop claimed); without `.focus()`,
  pointer selection works but keyboard shortcuts stay disabled.

## Root wiring (not owned by this packet)

1. `modules/ui/src/lib.rs`: add `pub mod selectable_text;`.
2. `modules/ui/Cargo.toml`: add `[[test]] name = "selectable_text",
   path = "../../tests/ui/selectable_text.rs"`.
3. Followup integration packet (separate session): route transcript user
   bodies and `MarkdownRenderer` blocks through `SelectableText`.

## Verification status

- Source-only packet: no builds or native gates per contract (root owns all
  gates). New drag/keyboard paths mirror vendor `InteractiveText` and
  in-tree `input.rs` / `command.rs` / `conversation_host.rs` call shapes.
- `tests/ui/selectable_text.rs`: 8 pure tests (clamp, normalize, drag
  lifecycle, unicode, stale-text invalidation, merge, keystrokes) + 4 GPUI
  tests (mount, `ctrl-a` select-all, `ctrl-c` clipboard write, unfocused
  copy gated). No-op-free; drag hit-testing itself follows the audited
  vendor path.
