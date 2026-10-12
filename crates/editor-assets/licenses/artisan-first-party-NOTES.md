# Artisan first-party artwork — provenance notes

Scope: `modules/assets/svg/artisan/*` — `app-icon.svg`, `star.svg`,
`logo-gradient.svg`, `success-check.svg`, `wordmark-stacked.svg`.

These are first-party Artisan product graphics:

- `app-icon.svg` (720×720) and `logo-gradient.svg` (720×720 gradient backdrop
  also serialized as a nested base64 `<image>` inside the app icon): the
  application identity art. The legacy tree additionally contains a sibling
  forge variant that is unreferenced (excluded from vendoring).
- `star.svg` (100×100): decorative mark used as a CSS luminance mask in the
  compaction model setting.
- `success-check.svg` (16×16 stroke path): static half of the onboarding
  success indicator; the legacy frontend draws it with a CSS stroke-dash
  animation (`t-check-draw`) which the native UI reimplements.

- `wordmark-stacked.svg` (136.675×225.15): the stacked AR/TIS/AN company
  wordmark as outlines, laid out exactly as the legacy `artisan-logo.svelte`
  set it (one size, −0.05em tracking, line heights 0.71/0.8/0.7). The
  letterforms are Sigurd (© Matthieu Salvaggio, Leon Hugues) at weight 400,
  taken from the six-glyph subset the legacy frontend shipped for this mark;
  the font itself is not vendored, only the outlined logo. Whether the
  typeface licence covers this logo use needs the same human attestation.

Origin kind is recorded as `local` with `needs_review = true`: placement,
naming, and first-party usage attest product ownership, but no in-tree
statement predates this inventory, so human attestation is required before any
external distribution claim. The repository's declared license for first-party
work is BSD-3-Clause (Cargo workspace `license` field).

Third-party tool credit preserved verbatim from excluded sibling file
`forge.svg`'s comment: "SVG created with Arrow, by QuiverAI
(https://quiver.ai)".
