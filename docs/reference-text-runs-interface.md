# Reference text runs — frozen interface

Status: frozen 2026-09-10. Owner: reference-text-runs worker
(`opencode-go/muse-spark-1.3-contributor`), worktree
`E:/artisan-editor-worktrees/wt-reference-text-runs`, branch
`codex/reference-text-runs`, base `f8ea8f0c`.

## Location

The reusable compiler lives in `artisan_ui::text_runs` (file
`modules/ui/src/text_runs.rs`) and is re-exported through
`artisan_ui::selectable_text`:

```rust
pub use crate::text_runs::{TextRunOverride, compile_text_runs};
```

Existing consumers keep the frozen `selectable_text::{TextRunOverride,
compile_text_runs}` path. Registration of the new module in
`modules/ui/src/lib.rs` (+ `BUILD`/`Cargo` if needed) is owned by the
root VP. The prose worker (`markdown_renderer`/theme/fonts) and the
renderer worker (`inline_code_text` + conversation surface) both use
this one function; no second typography compiler is allowed.

## API

```rust
use std::ops::Range;
use gpui::{HighlightStyle, Pixels, SharedString, TextRun, TextStyle};
use artisan_ui::selectable_text::{TextRunOverride, compile_text_runs, SelectableText};

pub struct TextRunOverride {
    pub range: Range<usize>,
    pub font_family: Option<SharedString>,
    pub letter_spacing: Option<Pixels>,
}

pub fn compile_text_runs(
    text: &str,
    default: &TextStyle,
    highlights: &[(Range<usize>, HighlightStyle)],
    overrides: &[TextRunOverride],
) -> Vec<TextRun>;

impl SelectableText {
    pub fn with_text_run_overrides(self, overrides: Vec<TextRunOverride>) -> Self;
}
```

`TextRunOverride` fields are `None`-inherits / `Some`-replaces:
`font_family: None` keeps the inherited family, `Some(family)` replaces
it for the whole range; `letter_spacing: None` keeps the inherited
spacing, `Some(spacing)` replaces it — including `Some(px(0.0))`, which
is how mono/code ranges normalize to zero tracking. Code weight (400)
travels in the highlight ranges, body weight (410) stays inherited;
neither lives in this struct.

## Semantics

`compile_text_runs` is a pure, windowless compiler. It unions the
boundaries of the highlight ranges and the override ranges, then emits
one run per atomic segment as `default.highlight(segment).to_run(len)`
with the active `font_family` / `letter_spacing` override applied.
Adjacent runs with identical shaping properties are coalesced.

Highlight contract: callers pass sorted, non-overlapping highlight
ranges, as produced by the Markdown seam and by
`merge_selection_highlight`. Anything else is still accepted without
panicking: unsorted input is normalized by position, and overlapping
ranges resolve through the existing vendor `gpui::combine_highlights`
sweep. Overlaps that agree on discrete properties (weight, style) merge
deterministically; overlaps with conflicting discrete properties
resolve in vendor fold order, so which one wins is unspecified — keep
conflicting highlight ranges disjoint. Overlapping overrides keep the
earliest range and drop the later one after sorting by start
(deterministic). Guarantees:

- Exact coverage: run lengths sum to `text.len()`; concatenated runs
  reproduce the plaintext bytes. Copy/hit-test inputs never change.
- Valid UTF-8: every boundary is a character boundary; the text itself
  is never re-sliced or reordered.
- Selection-safe: the caller merges the selection wash into
  `highlights` first (existing `merge_selection_highlight`, which only
  touches foreground/background), so selecting never changes family,
  spacing, weight, or style — glyph metrics are identical during select.
- Weight/link preservation: `TextStyle::highlight` carries weight,
  style, underline, and strikethrough; overrides only replace family and
  spacing.

`SelectableText::with_text_run_overrides` stores the overrides and
`request_layout` compiles `merged_highlights + window.text_style() +
overrides` into `StyledText::with_runs`. With no overrides the output
matches the previous `with_highlights` rendering exactly.

## Fail-closed validation (never panics on caller ranges)

A range is used only when it is ordered, fully in-bounds, and has both
endpoints on character boundaries. Anything else — reversed, empty,
out-of-bounds, or mid-character — is dropped entirely, never clamped,
so a bad caller range cannot shift surviving text either way and the
plaintext is never altered. Overlapping overrides keep the earliest
range and drop the later one after sorting by start (deterministic).
Empty text yields zero runs.

## Usage (renderer worker)

```rust
let overrides = vec![TextRunOverride {
    range: code_range,
    font_family: Some(mono_family),
    letter_spacing: Some(px(0.0)),
}];
let runs = compile_text_runs(&source, window.text_style(), &highlights, &overrides);
```

## Non-goals (explicit)

- No per-range font size: GPUI `TextLayout` lays out at one uniform
  size, so mixed sizes are out of scope for this packet.
- No vendor changes: `apply_font_family_overrides` (which recolors runs
  without splitting them) is untouched; the emoji worker owns vendor
  cosmic changes.
- No GPUI parser duplication and no replacement of the selection
  element: plaintext, retained selection, copy, highlight weights, and
  link effects are preserved.
