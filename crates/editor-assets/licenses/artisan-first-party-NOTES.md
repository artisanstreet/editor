# Artisan first-party artwork — provenance notes

Scope: `crates/editor-assets/svg/artisan/*` (`starburst.svg`,
`wordmark-stacked.svg` and the `rail-*.svg` layers) and
`crates/editor-assets/art/mountain.png`.

- `wordmark-stacked.svg` (136.675×225.15): the stacked AR/TIS/AN company
  wordmark as outlines, laid out exactly as the legacy `artisan-logo.svelte`
  set it (one size, −0.05em tracking, line heights 0.71/0.8/0.7). The
  letterforms are Sigurd (© Matthieu Salvaggio, Leon Hugues) at weight 400,
  taken from the six-glyph subset the legacy frontend shipped for this mark.
  Whether the Sigurd licence covers this logo use is unconfirmed; see
  `sigurd-NOTES.md`.

- `rail-*.svg` (24×24 stroke paths): the sidebar rail's New thread,
  Marketplace and Usage icons split into layers so they can animate on hover.
  The pen, page, bag, handle and level-cell paths are cut unchanged from
  Tabler Icons' `edit`, `shopping-bag` and `chart-column` (MIT, see
  `tabler-MIT.txt`); the bag's three items (dot, spark, gem) are first-party.

- `art/mountain.png` (1983×793): the mountain artwork behind the sign-in card,
  copied unchanged from the auth site (`web/apps/auth/static/images/art/
  mountain-1.png`). The Editor dithers it at runtime exactly as the site does.

Licence: first-party code and artwork in this repository are licensed under
the BSD-3-Clause licence in `LICENSE.md`. The exceptions above keep their own
terms: the Sigurd letterforms in `wordmark-stacked.svg` and the Tabler paths in
`rail-*.svg`.
