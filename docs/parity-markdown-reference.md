# Parity Markdown reference (native list-fidelity lane)

> Historical build notes: the current build and test workflow is documented in the [Cargo/Nix runbook](runbooks/native-dev.md).

Worker note for the bounded native Markdown fidelity correction. Owns only
`modules/ui/src/markdown.rs`, `modules/ui/src/markdown_renderer.rs`,
`tests/ui/markdown_seam.rs`, and this note. No caller was edited; the test
target registration stays root-owned.

## Fault

The canonical wide capture
`parity-proof-longform-wide-…png` drops the entire three-item assistant
checklist after "Remaining checks". Root cause: `markdown.rs` deliberately
omitted tight-list text in Phase 1 ("Container constructs are represented
only when `pulldown-cmark` exposes inner paragraph or heading events …
deliberately omitted instead of guessed"), so the tight
`- narrow … / - wide … / - [runbook](…) attached` list produced zero
blocks. The renderer then painted heading + prose + fence with no list.

## Electron truth (read-only)

- `modules/frontend/src/lib/components/markdown/content.svelte`: `comark`
  parser + `ComarkRenderer`; settled and streaming plugins; open-fence body
  stays plain mid-stream; rich math/Mermaid join only at settle.
- `modules/frontend/src/lib/components/markdown/parsing.ts`:
  `conversation_parse_options = { html: false }` — assistant output is
  untrusted, raw HTML stays inert text.
- `modules/frontend/src/lib/components/markdown/anchor.svelte`: links are
  untrusted; only `http:`/`https:`/`mailto:` (resolved against a base, so
  relative links pass there) become live anchors; anything else renders as
  plain children. `link-url.ts` further gates rich-link metadata to absolute
  HTTP(S).
- `lib/styles/prose.css`: bullets/counters muted, body muted-foreground,
  headings/bold foreground, plain links foreground underlined;
  conversation links (`a.conversation-link`, the only class the renderer
  emits) blue with no underline; inline code keeps backticks out
  (`content-none`).

## Native mapping

| Reference | Native |
|---|---|
| `comark` grammar | `pulldown-cmark 0.13.4` event stream only; no parallel parser, no first-party grammar |
| `{ html: false }` inert HTML | `Span::Html` / `Block::Html` carried verbatim; never interpreted or rendered as markup |
| tight/loose/ordered/nested/task lists | `Block::List { ordered, start, items, range }` with `ListItem { blocks, task }`; tight item text settles as one paragraph so tight and loose share the renderer path |
| task markers | engine enables exactly `Options::ENABLE_TASKLISTS` (which only affects list-item marker scanning); without it `pulldown-cmark` never emits `TaskListMarker` |
| emphasis/strong/link labels | `Span::Emphasis/Strong/Link`; images keep flattening into alt text |
| anchor `safe_href` guard | engine keeps every destination verbatim; only absolute `http(s)`/`mailto:` destinations become live links. Relative paths have no project base in the renderer, so they keep their plain label instead of opening an arbitrary local path; other schemes stay inert |
| open-fence plain body | `CodeFence { closed: false, tokens: None }` renders as unhighlighted code inside the fence chrome; unclosed fences still arrive balanced through the event stream. The rest of the message keeps rendering (see "Fences without tokens") |
| GFM tables | engine enables `Options::ENABLE_TABLES`; `Block::Table(Table { alignments, header, rows, range })` with inline-only `TableCell { spans }` (see "Tables") |
| `syntect` highlight ranges | unchanged `CodeToken` byte ranges over `CodeFence::source`, ordered/non-overlapping; unknown/open fences stay `None` |

## Renderer

- `render_block` gains `Block::List`: native `flex_col` with one `flex_row`
  per item, muted marker (`•`, `1.`, `☐`/`☑`), content recurses with depth
  indent. No HTML list elements.
- `present_inline` flattens spans into one `StyledText` through a single
  `with_highlights` call (`StyledText::with_highlights` replaces stored
  highlights, so chained calls would discard code and bold). Styles
  propagate recursively in one linear pass — each nested run inherits its
  parent style combined with the existing `HighlightStyle::highlight`
  helper, leaf text emits only non-default runs, and adjacent equal runs
  coalesce — so formatted-run count never goes quadratic on large replies
  the renderer re-parses per render: inline code reads muted with no wash,
  strong adds `FontWeight::SEMIBOLD` (plugin 600) in the foreground token,
  emphasis adds `FontStyle::Italic`, openable links read reference blue
  (`banner_info`: blue-500 light / blue-400 dark) at `FontWeight::MEDIUM`
  (plugin `a` 500) with no underline (`conversation-link` class
  semantics). Nested `a strong` / `a code` inherit the link color per the
  plugin rules.
- Paragraphs with openable links render as retained `SelectableText`
  whose `.links()` activation opens the clicked destination through
  `cx.open_url` (platform browser); the index is bounds-checked against
  the exposed link metadata. The selection element owns link clicks and
  suppresses drag activation, so no separate click handler lives beside
  it. Link-free runs stay selection-capable without link ranges.
- `InlinePresentation.links` exposes openable link ranges plus verbatim
  destinations as one metadata source for click handling and the
  selection consumer.

## Selection consumer wiring

- Every text leaf renders through retained
  `artisan_ui::selectable_text::SelectableText::retained(id, text, theme,
  base_highlights)` with `.links(ranges, callback)` exactly where links
  are openable: paragraph and heading runs, code blocks (syntax token
  highlights as base, no links), inert HTML blocks (verbatim, no
  highlights), and the plain-source fallback. No `StyledText` or
  `InteractiveText` output remains in this renderer.
- Ids are selector-derived per block, inline run, and code leaf
  (`{selector}-plain`, `{selector}-html`, `{selector}-code-text`, and the
  block selector itself for inline runs): positional, never built from
  label text or destinations, so no identity collisions. No caller state,
  focus maps, caches, or sidecars — retention lives in framework element
  state under those ids.
- Typography, wrap, font family, code background, syntax colors, and
  layout are unchanged: the same container divs wrap the new leaves, and
  `present_inline` metadata plus the allowlist are untouched, so the
  preceding gate's nesting/style evidence still binds.
- The `selectable_text` module itself is integrated by root; this lane
  only consumes its frozen API (`retained(...).links(...)`), which is
  unchanged by root's internal import/perf fix.
- Open or unhighlighted fences render as plain code in the fence card
  (see "Fences without tokens").

## Reference prose typography (frozen shared helper)

- `artisan_ui::theme::ProseTypography` (plus `ProseHeading`) is the frozen
  recipe for conversation prose AND workspace surfaces. Shell
  (`thread_screen`, `native_composer`) and renderer (`conversation_surface`,
  `inline_code_text`) consumers read these instead of re-deriving:
  - body: `BODY_SIZE_PX` 16, `BODY_LINE_PX` 28, `BODY_WEIGHT`
    `FontWeight(410)`, `BODY_TRACKING_PX` −0.64; `body_tracking_px(size)`
    resolves −0.04 em at any size (14 px workspace text takes −0.56);
  - headings: `HEADING_WEIGHT` `FontWeight(630)`, `heading(level)` returns
    size/line/tracking/margins — h1 30/37.5/−1.35/0/26.667, h2
    24/30/−1.08/48/24, h3 20/27.5/−0.9/32/12, h4–h6 18/24.75/−0.81/27/9
    (plugin em margins at the overridden sizes; first children take no top
    margin, `h2/h3/h4 + *` zeroes only the follower top so the heading
    bottom still collapses through);
  - inline: `STRONG_WEIGHT` SEMIBOLD, `LINK_WEIGHT` MEDIUM,
    `CODE_SIZE_PX` 14 / `CODE_LINE_PX` 24;
  - blocks: paragraphs 20, fences 24, lists 20, indent 26, item pitch 8,
    item paragraphs 12, nested lists 12, fence padding 16;
  - fence radius has deliberately NO constant: the fence shares the
    composer card's 18 px, the workspace ramp's `2xl` step, so renderers
    use `RadiusTokens::value(RadiusStep::X2l)`;
  - gaps collapse top-only via `block_gaps(blocks, scope) -> Vec<f32>`
    (`BlockScope::Root/Item`): each gap renders once as top margin because
    flex columns never collapse, so two paragraphs read max(20, 20) = 20,
    never 40; `h2/h3/h4 + *` clears the follower top only, so h2+paragraph
    still reads the heading's 24 px bottom.
- Families are unchanged pending the open question: Spline Sans body and
  headings, Spline Sans Mono code, Artisan Neo wordmark at 600.
  `editor_text_desktop` is untouched; prose behavior applies at the
  renderer/helper only.
- New vendored faces `spline-sans-410.ttf` / `spline-sans-630.ttf`
  (true `instantiateVariableFont` instances, OS/2 410/630, outlines
  distinct from 400/600 neighbors) are registered through the existing
  `static_faces` list; Bazel runfiles entries stay root-owned.
- Honest gaps, no fakes: inline code reads 400 muted; mono face and
  normal tracking ride the frozen text-run contract
  (`InlinePresentation.code_ranges` → `TextRunOverride` with the mono
  family and zero tracking, wired in `render_inline`). Size deliberately
  has no override in that API, so inline code keeps the inherited 16 px
  instead of the reference 0.875 em (14 px) — a stated limit, not parity.
  Shared `StyledText` already supports `with_font_family_overrides`
  (sorted non-overlapping char-boundary ranges) and per-run
  `TextRun.letter_spacing`, and the renderer consumes them only through
  the shared selection element — never edited here. Until the dependency
  integrates (root first, then gate), code spans inherit body face/size/
  tracking; blockquote/hr
  structure has no engine model in this packet (`markdown.rs` frozen) so
  quotes read as paragraphs and rules are dropped — both reported for a
  follow-up engine packet; fence copy/filename chrome has no renderer
  action counterpart.
- Body, headings, and strong carry their reference colors (muted body,
  foreground headings/strong) instead of inheriting the bright parent;
  fences read the foreground pre-code token on the composer's glass card
  (see "Fence card"), with copy/filename chrome having no renderer action
  counterpart.

## Fence card

A fence paints as the composer's glass card, by product direction rather
than reference parity (the reference snippet is an opaque vertical gradient
at `rounded-3xl` under the `card-lg` shadow):

- Material: `artisan_ui::glass`, the one recipe the composer, pickers, and
  menus share — foreground lift, quiet diagonal material, quiet highlight,
  and the `card-glass` edge stack — at the composer's 18 px radius.
- No backdrop blur: a fence scrolls over the flat transcript canvas, where
  a blur pass per fence changes no pixel.
- Width: the full prose column, whatever the code's own width.
- Height: exactly its lines. The engine keeps the fence body verbatim,
  ending in the line terminator of its last line; the renderer drops that
  one terminator for display, since a text element paints it as an extra
  empty line. Blank lines the author wrote before it stay.

## Fences without tokens

A fence carries `tokens: None` while it is still open (streaming), when it
has no info string, when it is an indented block, and when its language has
no bundled `syntect` grammar. The renderer used to treat any such top-level
fence as unrepresentable and drop the **whole message** to the raw-source
fallback, so every `**`, `_`, heading, and list marker in that message
showed literally — and a streaming reply flipped to raw source for as long
as its fence stayed open. `render_code` always handled missing tokens, so
the fallback was removed: the fence reads as plain code in its chrome and
everything around it renders. Only a parse failure, or a non-empty body
that produced no blocks, still takes the raw-source fallback.

`parse_document` highlights closed fences at every depth, including fences
inside list items, which previously never received tokens.

## Tables

- Engine: `modules/ui/src/markdown/table.rs` owns `Table`,
  `TableAlignment`, `TableCell`, and the accumulator. Cells assemble in the
  shared inline buffer, so emphasis, code, and links nest exactly as in a
  paragraph. `pulldown-cmark` pads short rows and drops excess cells, so
  every row carries `alignments.len()` cells.
- A header row is a paragraph until its delimiter row arrives; a streaming
  table therefore reads as one pipe-delimited line for a moment and then
  settles into the grid.
- Renderer: `modules/ui/src/markdown_renderer/table.rs` lays the cells out
  as one grid with content-sized tracks (`grid_cols_max_content`:
  `minmax(0, max-content)`). Short columns hug their content; when the row
  outgrows the message width the tracks shrink and cells wrap. Column
  alignment places the hugging cell content on the authored edge.
- Type follows the plugin `table`: 14 px / 24 px, header 600 in the
  foreground token, `border` hairlines under the header and between rows,
  8 px cell padding with flush outer edges, 28 px block margins
  (`ProseTypography::TABLE_*`).
- Deliberate difference: the reference table fills its container
  (`width: 100%`, automatic table layout). GPUI grids only offer uniform
  track templates, so the native table hugs its content at the leading
  edge instead of stretching every column equally. There is no horizontal
  scroll; a table with more columns than the width can carry wraps tightly.

## Streaming / balanced-events contract

- `pulldown-cmark` emits balanced `Start`/`End` pairs even for truncated
  input (unclosed fences still close as open; unmatched emphasis/link
  delimiters stay literal text), so the builder carries no end-of-input
  recovery layer: debug builds assert every stack is empty after the event
  loop. No malformed parser events are invented.
- Byte ranges stay honest: confirmed tags use event offsets; tight
  pre-nest flushes reuse the item start rather than inventing an end.
- Source authority: the builder only moves `pulldown-cmark` text; it never
  synthesizes copy.

## Evidence

`tests/ui/markdown_seam.rs` (engine seam plus the pure `present_inline`
helper; renderer pixels stay root's capture):

- exact `LONGFORM_ASSISTANT_BODY` checklist: 3 items, texts, `runbook`
  destination, plus undisturbed heading/prose/fence;
- exact `LONGFORM_USER_BODY` list: code span + reference-link preservation;
- ordered (`Some(1)`), loose (paragraph blocks), nested exact tree (2
  outer items; first outer owns exactly its paragraph plus a 2-item nested
  list), task (`Some(false)`/`Some(true)`) lists;
- emphasis/strong/link structure with destinations;
- merged highlights: code+bold+link in one paragraph keep all three runs;
  nested bold code and bold link labels combine styles in single segments;
  emphasis presents italic; relative/unsafe links expose no click metadata
  or affordance; `mailto:` opens like HTTP;
- truncated streaming prefixes (cut list item, unclosed strong/link,
  unterminated fence) with single-occurrence no-drop assertions.

## Known limits (root gates)

- No `cargo`/native gate ran in this worker (source-only lane); build,
  tests, and pixel comparison are root-owned. Two gate failures drove this
  correction (`TagEnd::List(bool)` tuple pattern, `UnderlineStyle.color`
  needing `.to_paint()` to `Hsla`); both are fixed in-tree.
- Blockquotes, strikethrough, superscript/subscript still pass their inner
  text through without dedicated blocks (unchanged Phase 1 scope); task
  lists and tables are the enabled GFM set.
- Emphasis follows `CommonMark` flanking rules, so a closing `**` that
  follows punctuation and touches a letter (`**Note:**text`,
  `**"quoted"**text`) stays literal. `pulldown-cmark` owns that grammar and
  no first-party rewrite exists.
- Text inside a raw HTML block (a line opening with a block tag such as
  `<details>`, up to the next blank line) is inert, so Markdown markers in
  it stay literal. The reference parses that text because `html: false`
  disables HTML blocks outright; `pulldown-cmark` has no such switch.
- Fences in languages outside the bundled `syntect` set (`ts`, `tsx`,
  `svelte`, `toml`, `jsx`, `nix`, …) render unhighlighted.
- Task markers render as `☐`/`☑` glyphs, not interactive checkboxes.
