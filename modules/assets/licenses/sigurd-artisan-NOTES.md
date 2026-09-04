# Sigurd Artisan wordmark face — provenance notes

Scope: `modules/assets/fonts/sigurd-artisan.woff2` — the `"Sigurd Variable"`
face declared in the legacy `@font-face` block
(`modules/frontend/src/lib/styles/fonts.css:31–37`, weight range 300–900)
and worn by the product wordmark
(`modules/frontend/src/lib/components/artisan-logo.svelte:25`,
`style:font-family="'Sigurd Variable', serif"`).

The file name carries an `artisan` suffix (`sigurd-artisan.woff2`), which
attests a custom cut prepared for this product, but no in-tree license
statement, receipt, or foundry grant was found for it in the legacy
repository: the only references are the `@font-face` declaration and the
logo call site above. It is therefore recorded as first-party-adjacent with
the same caveat as `artisan-first-party-NOTES.md`: placement, naming, and
first-party usage attest product ownership, but human attestation is
required before any external distribution claim. The repository's declared
license for first-party work is BSD-3-Clause (Cargo workspace `license`
field).

If attestation instead shows a commercial foundry grant, replace this note
with the grant text and record the SPDX expression in `FONTS.md`.
