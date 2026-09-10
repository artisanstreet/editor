# Parity Markdown reference (native list-fidelity lane)

Worker note for the bounded native Markdown fidelity correction. Owns only
`modules/ui/src/markdown.rs`, `modules/ui/src/markdown_renderer.rs`,
`tests/ui/markdown_seam.rs`, and this note. No caller was edited.

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
  relative links pass) become live anchors; anything else renders as plain
  children. `link-url.ts` further gates rich-link metadata to absolute
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
| emphasis/strong/link labels | `Span::Emphasis/Strong/Link`; images keep flattening into alt text |
| anchor `safe_href` guard | engine keeps every destination verbatim; `is_safe_link_destination` in `markdown_renderer.rs` allows absolute `http(s)`/`mailto:` plus relative/scheme-relative paths, plain-label fallback otherwise; destinations never execute |
| open-fence plain body | `CodeFence { closed: false, tokens: None }` + renderer plain fallback; EOF recovery settles unterminated fences open instead of dropping the tail |
| `syntect` highlight ranges | unchanged `CodeToken` byte ranges over `CodeFence::source`, ordered/non-overlapping; unknown/open fences stay `None` |

## Renderer

- `render_block` gains `Block::List`: native `flex_col` with one `flex_row`
  per item, muted marker (`•`, `1.`, `☐`/`☑`), content recurses with depth
  indent. No HTML list elements.
- `render_inline` flattens all spans into one `StyledText`: existing
  code style unchanged; strong adds `FontWeight::BOLD`; safe links add
  accent color + 1px underline (`UnderlineStyle`, both already used
  elsewhere in the tree); emphasis keeps its structure in the model and
  paints plain (no italic primitive in the pinned GPUI build).
- `block_needs_plain_fallback` unchanged: open or unhighlighted fences
  still take the plain body path.

## Streaming / malformed contract

- Truncated lists, unclosed emphasis/strong/link frames, unclosed
  paragraphs/headings, open fences, and open HTML all flush at EOF with no
  dropped confirmed text and no duplicates; unclosed link destinations stay
  out of visible copy.
- Byte ranges stay honest: confirmed tags use event offsets; EOF-synthesized
  closes end at `source.len()`; tight pre-nest flushes reuse the item start
  rather than inventing an end.
- Source authority: the builder only moves `pulldown-cmark` text; it never
  synthesizes copy.

## Evidence

`tests/ui/markdown_seam.rs` (engine seam only; renderer pixels stay root's
capture):

- exact `LONGFORM_ASSISTANT_BODY` checklist: 3 items, texts, `runbook`
  destination, plus undisturbed heading/prose/fence;
- exact `LONGFORM_USER_BODY` list: code span + reference-link preservation;
- ordered (`Some(1)`), loose (paragraph blocks), nested (child list inside
  parent item), task (`Some(false)`/`Some(true)`) lists;
- emphasis/strong/link structure with destinations;
- unsafe `javascript:` destination kept inert in the model with its label;
- truncated streaming prefixes (cut list item, unclosed strong/link,
  unterminated fence) with single-occurrence no-drop assertions.

## Known limits (root gates)

- No `cargo`/native gate ran in this worker (source-only lane); build,
  tests, and pixel comparison are root-owned.
- Emphasis paints plain until GPUI exposes italics; structure is preserved
  for that upgrade.
- Blockquotes, tables, strikethrough, superscript/subscript still pass
  their inner text through without dedicated blocks (unchanged Phase 1
  scope); GFM stays disabled.
- Task markers render as `☐`/`☑` glyphs, not interactive checkboxes.
