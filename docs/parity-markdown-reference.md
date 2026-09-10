# Parity Markdown reference (native list-fidelity lane)

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
  headings/bold/links foreground, conversation links blue with underline
  offset; inline code keeps backticks out (`content-none`).

## Native mapping

| Reference | Native |
|---|---|
| `comark` grammar | `pulldown-cmark 0.13.4` event stream only; no parallel parser, no first-party grammar |
| `{ html: false }` inert HTML | `Span::Html` / `Block::Html` carried verbatim; never interpreted or rendered as markup |
| tight/loose/ordered/nested/task lists | `Block::List { ordered, start, items, range }` with `ListItem { blocks, task }`; tight item text settles as one paragraph so tight and loose share the renderer path |
| task markers | engine enables exactly `Options::ENABLE_TASKLISTS` (which only affects list-item marker scanning); without it `pulldown-cmark` never emits `TaskListMarker` |
| emphasis/strong/link labels | `Span::Emphasis/Strong/Link`; images keep flattening into alt text |
| anchor `safe_href` guard | engine keeps every destination verbatim; only absolute `http(s)`/`mailto:` destinations become live links. Relative paths have no project base in the renderer, so they keep their plain label instead of opening an arbitrary local path; other schemes stay inert |
| open-fence plain body | `CodeFence { closed: false, tokens: None }` + renderer plain fallback; unclosed fences still arrive balanced through the event stream |
| `syntect` highlight ranges | unchanged `CodeToken` byte ranges over `CodeFence::source`, ordered/non-overlapping; unknown/open fences stay `None` |

## Renderer

- `render_block` gains `Block::List`: native `flex_col` with one `flex_row`
  per item, muted marker (`•`, `1.`, `☐`/`☑`), content recurses with depth
  indent. No HTML list elements.
- `present_inline` flattens spans into one `StyledText` through a single
  `with_highlights` call (`StyledText::with_highlights` replaces stored
  highlights, so chained calls would discard code and bold). Nested
  combinations sweep into atomic segments combined with the existing
  `HighlightStyle::highlight` helper, then coalesce: code keeps its wash,
  strong adds `FontWeight::BOLD`, emphasis adds `FontStyle::Italic`,
  openable links add accent color + 1px `UnderlineStyle`.
- Paragraphs with openable links render as `InteractiveText` whose
  `on_click` opens the clicked destination through `cx.open_url` (platform
  browser); the index is bounds-checked against the exposed link metadata.
  Link-free runs stay plain `StyledText`.
- `InlinePresentation.links` exposes openable link ranges plus verbatim
  destinations as one metadata source for click handling and the future
  selection consumer.
- `block_needs_plain_fallback` unchanged: open or unhighlighted fences
  still take the plain body path.

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
- Blockquotes, tables, strikethrough, superscript/subscript still pass
  their inner text through without dedicated blocks (unchanged Phase 1
  scope); only task lists join the enabled set.
- Task markers render as `☐`/`☑` glyphs, not interactive checkboxes.
