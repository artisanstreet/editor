# Reference layout audit — overlay composer, endspace, prose wrapping

Scope: read-only except where §4 assigns implementation (shipped this packet
in the listed files). Canonical reference is
`C:/Users/sander/Desktop/artisan-editor` (read directly; the seven cited
reference files are byte-identical to the worktree copies by SHA256).
Native is `42c24b57` moving to this packet
(`thread_screen.rs`, `native_composer.rs`, `conversation_surface.rs` read).
No pixel claims.

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

**Main workspace inheritance** (`utilities.css:708-724`, `prose.css:728-739`,
`sectioned-panel.svelte:310`): the surfaces row wears `docs-responsive-surfaces`,
so workspace surfaces inherit body `font-weight: 410`, `letter-spacing: -0.04em`
(`code/kbd/samp/pre` reset to normal) and headings `630`/`-0.045em`. The 410
styling exists as ground truth (only font *families* stay pending — keep Spline,
no reference-face switch). Native status: unapplied today (400/500, no body
tracking); specified future, unassigned. Expressibility on record: GPUI
`FontWeight` is a tuple struct over `f32`, so 410 is expressible; `-0.04em`
resolves per size in px at application.

**Composer overlay** (`thread-composer.svelte:526-595`): outer frame
`prose-column-frame pointer-events-none absolute inset-x-0 bottom-0 z-20 flex
col items-center gap-2 pb-4 sm:pb-6`. The `sm:` step keys off the **window
viewport (≥640 px)**, so the inset is responsive: **24 px at/above 640 px,
16 px below**. Jump circle, failure alert, and LipCard are
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
- D4 — bottom inset 16 flat vs reference responsive 24/16 (`sm:pb-6` on the
  window viewport; no native minimum width is enforced, so no ≥640 assumption).
- D5 — jump affordance lives in the transcript surface, not the overlay frame.
- D6 — assistant body width pinned 672 (balanced-only) instead of responsive
  prose − 96 (user bubble 576 is already exact).
- Correct already, do not touch: card frame (min-h-128, p-8, r-18 quiet glass),
  shell gap-8 stack, prose centering/`max-w`, inspector conditional column,
  black shell/titlebar/sidebar, Artisan Neo 600 wordmark (20 px, −1.0 px),
  Spline faces (font question pending — keep).

## 3. Restoration spec (fidelity, not caps)

- Overlay dock in `thread_screen.rs`: absolute bottom-anchored frame
  (`inset-x-0 bottom-0`, responsive `pb-24/16` from live window bounds),
  content-sized height, plain container
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
   responsive inset + prose gutter equivalence. Tests: mounted bounds — inner
   card anchors by the viewport rule (24 wide / 16 narrow, real window
   bounds), frame strictly overlaps the full-height transcript, 12-line draft
   grows the card while transcript height is unchanged, narrow + wide + resize.
   SHIPPED this packet.
2. Composer lane — `modules/frontend/src/native_composer.rs` (+ lip owner file
   for the variant): remove cap/scroll, lip variant. Tests: editor grows past
   240 uncapped (288px card math in-file), min-h-64 kept, no internal scrollbar.
   SHIPPED this packet (cap/scroll removal + growth test; lip variant stays
   with the lip owner).
3. Transcript lane — `modules/frontend/src/conversation_surface.rs` +
   `modules/frontend/src/shell_layout.rs` (pure base/growth): endspace element
   + formula. Tests: unit (base 192, anchored growth, floor) + layout (tail
   clears a tall overlay, scroll-to-bottom lands on content, mount opens at
   latest).
4. No touch: manifests/lockfiles, `theme.rs`, fonts, `desktop_shell.rs` chrome,
   wordmark, transport/queue/draft/recovery, Svelte sources.

## 5. Audit provenance (one correction, now folded in)

An earlier draft of this note claimed `410/-0.04em` was missing from the
reference, based on directory searches that matched nothing. That claim is
retracted: re-reading the authoritative checkout directly shows the tokens
plainly at the lines cited in §1, and the seven reference files are
byte-identical across checkouts. No active statement in this note disputes the
410 styling's existence; §1 states it as ground truth. Nothing else in §1–§4
changed, because those sections were read from identical bytes throughout.

## 6. Frozen host/surface spacer contract (for the surface worker)

The overlay restores the card; tail clearance needs the transcript spacer the
shell lane must not invent. Existing APIs the surface integration builds on
(read-only freeze, no edits here):

- `ConversationSurface::render` transcript assembly
  (`conversation_surface.rs:3796-3822`): turn loop appending to the `transcript`
  flex column, wrapped by `ScrollArea` at 3886. Endspace div goes after the
  loop, inside the scroll area — that file's owner places it.
- `scroll_handle()` (1034), `scroll_to_bottom()` (1112),
  `set_jump_to_latest_visible()` (1101): existing scroll/jump contract, unchanged.
- `ScrollAnchorRegistry` + painted custody (3801-3874) and
  `drain_painted_scroll_targets`: anchor identities key off turn/item ids, so
  an appended spacer cannot disturb them; scroll math targets content bottom,
  which the spacer extends.
- `ConversationHost::{controller_view, dispatch, canonical_snapshot}`: the
  established read/project path (already used by the activity replay).
- Policy precedent: `ConversationBaseEndSpacePixels` 192 and
  `ConversationEndSpaceHeight` growth stay the formula; recommend pure
  constants in `shell_layout.rs` beside the inspector clamp.

## 7. Shell status — pending surface counterpart

This lane ships the overlay + uncapped editor with no spacer of its own. Until
the surface lane ships endspace, scroll-to-bottom lands the tail under the
overlay card: known, accepted, and reported here — not silently fixed with a
shell-owned fake spacer or a reinstated cap.
