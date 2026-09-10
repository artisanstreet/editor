# Reference layout audit — overlay composer, endspace, prose wrapping

Scope: read-only. Canonical reference is `C:/Users/sander/Desktop/artisanstreet/editor`
(verified by hash: `sectioned-panel.svelte` and `thread-composer.svelte` byte-identical
to the worktree copies; `thread-workspace.svelte` identical modulo line endings).
Native is `42c24b57` (`thread_screen.rs`, `desktop_route_body`, `native_composer.rs`,
`conversation_surface.rs`). No source edits. No pixel claims.

## 1. Reference facts (frozen)

**Frame** (`sectioned-panel.svelte:305-347`, `+layout.svelte:565-652`): rail `w-14`,
`main.p-2.pl-0`, surfaces row `gap-2`, primary `flex-1 rounded-3xl p-1`,
inspector `w-(--inspector-width)` only behind `{#if secondary}` (no reserved space).
`--inspector-width: clamp(16rem, 25vw, 350px)` (`theme.css:176`).

**Prose** (`theme.css:156-204`, `utilities.css:661-699`): balanced `--prose-width: 48rem`
(768; tight 672, loose 896); `--prose-body-width: prose − 6rem` (message inset);
`--prose-gutter: 2rem`; `--prose-rail-gap: 1rem`; `--prose-rail-margin` derived.
`.prose-column` default centers (`margin-inline: auto`); rail variant shifts surplus
left; `.prose-column-frame` bounds composer children to
`min(prose, 100% − 3rem)`. Transcript is full-bleed and insets its own text.

**Composer overlay** (`thread-composer.svelte:526-595`): outer frame
`prose-column-frame pointer-events-none absolute inset-x-0 bottom-0 z-20 flex
col items-center gap-2 pb-4 sm:pb-6`. Desktop is always ≥ sm, so the effective
bottom inset is **24 px**, not 16. Jump circle, failure alert, and LipCard are
each `prose-column w-full max-w-(--prose-width)` centered children.
LipCard: `radius-surface 2xl` (18 px), gap 8 (nested 10), `flex-col-reverse`,
glass-vs-solid variant by open state. Card body `min-h-32 p-2`; editor
`min-h-16 px-3 py-2 text-base` (16/24) with **no max-height and no internal
scroll** — growth pushes the overlay taller.

**Endspace** (`thread-workspace.svelte`, `scroll-position.ts:4,90-96`): transcript
ends with a measured spacer, base **192 px**
(`ConversationBaseEndSpacePixels`), growing while anchored
(`ConversationEndSpaceHeight = max(base, item_top + viewport − inset − space_top)`).
Scroll-to-bottom (`ConversationBottomScrollTop`) needs no overlay math because
the spacer clears the overlay. Mount opens at latest content (assignment, not
animated scroll).

**Bodies** (`conversation-message.svelte:187-249`): user bubble `max-w-xl`
right-aligned; assistant/status `max-w-(--prose-body-width)`.

## 2. Native deviations (all confirmed in code)

- D1 — static dock, no overlay: `thread_screen.rs render_open` docks the composer
  in-flow with `pb-16` (`COMPOSER_PAD_BOTTOM_PX`). Every keystroke steals
  transcript height; the 240 px editor cap exists only to bound that theft.
- D2 — invented editor cap: `native_composer.rs:2354-2373` documents it openly
  (`max_h 240` + `overflow_y_scroll`). Reference editor is uncapped.
- D3 — no endspace: `conversation_surface.rs` has no end spacer (grep: none);
  scroll-to-bottom targets raw content height. Restoring the overlay without
  endspace would bury the tail under the card.
- D4 — bottom inset 16 vs reference desktop 24 (`sm:pb-6` always applies natively).
- D5 — jump affordance lives in the transcript surface, not the overlay frame.
- D6 — assistant body width pinned 672 (balanced-only) instead of responsive
  prose − 96 (user bubble 576 is already exact).
- Correct already, do not touch: card frame (min-h-128, p-8, r-18 quiet glass),
  shell gap-8 stack, prose centering/`max-w`, inspector conditional column,
  black shell/titlebar/sidebar, Artisan Neo 600 wordmark (20 px, −1.0 px),
  Spline faces (font question pending — keep).

## 3. Restoration spec (fidelity, not caps)

- Overlay dock in `thread_screen.rs`: absolute bottom-anchored frame
  (`inset-x-0 bottom-0`, `pb-24`), content-sized height, plain container
  (GPUI non-interactive divs don't occlude; only the card/buttons intercept),
  painted after the transcript; children prose-width centered (jump circle,
  lip, card). Transcript keeps full main height on every keystroke and resize.
- Uncap the editor in `native_composer.rs`: remove `max_h 240` +
  `overflow_y_scroll`; keep `min-h-64 px-12 py-8 text-16/24`. Lip glass/solid
  variant by open state where the lip renders.
- Endspace in the transcript: spacer div after turn sections, base 192 with
  anchored growth per `ConversationEndSpaceHeight`; scroll-to-bottom math
  unchanged. Recommend pure constants/formula in `shell_layout.rs`, element in
  `conversation_surface.rs` (it owns the scroll area and anchor measurements).
- Jump circle moves into the overlay frame, driven by the surface's existing
  visibility/action plumbing (no new state ownership).
- D6 optionally: assistant `max-w` follows the active prose width (576/672/800).

## 4. Exact implementation ownership/files/tests

1. Shell lane — `modules/frontend/src/thread_screen.rs`: overlay frame +
   jump-circle placement + `pb-24`. Tests: mounted bounds — frame bottom-anchored
   to the route body, children centered on the reading column, transcript keeps
   full height as the editor grows, narrow + wide + resize.
2. Composer lane — `modules/frontend/src/native_composer.rs` (+ lip owner file
   for the variant): remove cap/scroll, lip variant. Tests: editor grows past
   240 uncapped, min-h-64 kept, no internal scrollbar.
3. Transcript lane — `modules/frontend/src/conversation_surface.rs` +
   `modules/frontend/src/shell_layout.rs` (pure base/growth): endspace element
   + formula. Tests: unit (base 192, anchored growth, floor) + layout (tail
   clears a tall overlay, scroll-to-bottom lands on content, mount opens at
   latest).
4. No touch: manifests/lockfiles, `theme.rs`, fonts, `desktop_shell.rs` chrome,
   wordmark, transport/queue/draft/recovery, Svelte sources.

## 5. Unresolved shorthand — `410/-0.04em`

Exhaustive search of canonical frontend src (`*.svelte`, `*.css`, `*.ts`) finds
no `410` and no `-0.04em`. Native wordmark is 20 px/600/−1.0 px (−0.05 em), not
−0.04 em. Only 410-adjacent number on record is the capture's composer center-x
(≈409 px physical). Nothing is frozen to it; needs user/root confirmation
before any number carrying that name enters code. It does not block §3.
