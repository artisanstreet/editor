# Shared emoji interface (native-first + Twemoji fallback)

Frozen integration contract. Read before touching either side. Base vendor
`7aaf67d755`; primary base `42c24b57`.

## 1. Verified font facts (measured, not claimed)

Official Mozilla Twemoji COLR v0.7.0 release asset `Twemoji.Mozilla.ttf`
(`github.com/mozilla/twemoji-colr/releases/download/v0.7.0/`, 1,474,284
bytes, SHA-256
`6d90152ee0d29e82fe2a87793af5aa4b7ad13e6538360889e141e81ed299ee8e`):

- PostScript nameID 6 (both platforms/langs): `TwemojiMozilla`.
- COLR version 0 + CPAL; single GSUB `ccmp` lookup with precomposed
  ZWJ-sequence layer glyphs (3,444 zwj/family-like names).
- cmap: 1,418 entries, emoji-only (`a` MISSING — cannot shadow body
  text). Covers U+1F389, U+2764, U+FE0F, U+200D, regional indicators,
  skin-tone modifiers, `#`, U+20E3, U+1F44D/U+1F468. `U+FE0E` (VS15)
  MISSING — text-presentation requests correctly fall through.
- Measured coverage is NOT a version boundary: U+1FAE0/U+1FAE1/U+1F426
  present, U+1F6DC/U+1FAE8 MISSING. Coverage is decided by shaping, never
  by Unicode-version guess.
- License (verbatim from the `v0.7.0` tag `LICENSE.md`): build code
  Apache-2.0 (Mozilla Foundation 2016-2018), emoji art CC-BY-4.0
  (Twemoji project). Both recorded in the asset provenance doc.

## 2. Vendor packet (`wt-parity-gpui-capture`, owns ONLY
`crates/gpui_wgpu/src/cosmic_text_system.rs`)

- Predicate: extend `check_is_known_emoji_font` with `"TwemojiMozilla"`,
  keeping `NotoColorEmoji`, `SegoeUIEmoji`, `AppleColorEmoji`,
  `.AppleColorEmojiUI`. No other predicate change.
- No shaping, fallback-chain, rasterizer, selector-policy, dependency,
  or family-stack changes. Twemoji participates only through the
  existing cosmic/fontdb fallback machinery; it is never requested as a
  primary family (the `load_family` no-`m` guard would drop it there —
  fallback-only by construction).
- Tests (all in-file, existing harness shapes, no execution in lane):
  predicate extension incl. `TwemojiMozilla` positive; platform-first:
  system fonts + Twemoji registered, U+1F389 still won by
  `SegoeUIEmoji`; Twemoji-wins: no-system-fonts db + IBM Plex body +
  Twemoji bytes, flag cluster shapes ONE cluster from Twemoji with the
  emoji flag and colored BGRA pixels; invariant sweep over flag, ZWJ
  family, keycap, skin-tone clusters (single face, single cluster,
  native preferred). Twemoji bytes load at test time from the
  superproject asset path with graceful skip when absent (no
  compile-time coupling, vendor tree stays standalone).

## 3. First-party asset packet (`wt-parity-selection`, additive only)

- New: `modules/assets/fonts/twemoji-mozilla.ttf` (byte-exact download
  above) + `modules/assets/fonts/TWEMOJI.md` (source URL, tag, SHA-256,
  measured coverage table, license identification, pointer to this
  interface). Prose worker owns `fonts.rs`/`FONTS.md`/manifest
  registration and theme — untouched here.
- No transcript/Markdown image substitution, no parser, no registry, no
  per-screen fallback lists.

## 4. Acceptance (root gates, Windows-scoped)

- Vendor gate green incl. new tests with the asset present (Twemoji
  proofs run and assert; a missing asset fails loudly on Windows, never
  silent-skips). Non-Windows configurations skip platform tests.
- Preserved: native party popper selects Segoe with Twemoji registered;
  body text stays on the body font; selection bytes unchanged (existing
  `selectable_text` + Markdown suites); VS15 text presentation never
  routes to Twemoji (presentation policy itself is a separate packet);
  Spline stacks untouched.
- Deterministic fixtures by measured coverage, never version guess:
  England tag flag (Segoe covers 1 of 6 scalars, Twemoji all 6 plus a
  precomposed ligature) proves Twemoji-wins; party popper, VS16, ZWJ
  family, skin tone, and keycap pin native-first under the established
  order; VS15 pins Twemoji non-interference.
- macOS is MISSING work, not claimed: native CoreText keeps Apple
  first through its own `is_emoji` allowlist
  (`crates/gpui_macos/src/text_system.rs:414-418`), but Twemoji
  fallback through CoreText cascade / `memory_source` registration
  (`:98-134`) is unverified — no macOS test here proves it. Root
  integration followup required. Linux inherits the shared cosmic
  predicate (Noto path intact).
- macOS: native CoreText keeps Apple first (`MacTextSystem`
  `is_emoji` allowlist already covers `AppleColorEmoji`); Twemoji
  fallback there flows through CoreText cascade / `memory_source`
  registration — unproven by Windows tests, reported as root
  integration followup, not claimed here.
