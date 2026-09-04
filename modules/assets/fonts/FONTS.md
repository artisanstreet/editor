# Bundled typefaces — manifest

The four `@font-face` families the legacy editor actually declares
(`modules/frontend/src/lib/styles/fonts.css:7–37`) and spends through the
theme's `--font-*` tokens (`theme.css:313–315`, `fonts.css:39–42`) and
`typography_css_stacks` (`modules/frontend/src/lib/appearance/typography.ts`,
defaults `text: "Artisan Neo"`, `code: "JetBrains Mono"`).

Windows format constraint: DirectWrite's in-memory font loader rejects WOFF2
(`DWRITE_E_FILEFORMAT`, observed at startup as `0x88985000`), so every file
below is TrueType. Where the legacy tree ships a TTF it is vendored
bit-identical; otherwise the file is the lossless WOFF2 decompression of the
legacy `@font-face` source (WOFF2 is a lossless container, so the outlines
are identical — only the wrapper changed). Lengths and SHA-256 digests below
pin that identity.

Geist (`fonts/geist/*`) and Neue Montreal (`fonts/neue-montreal/*`) ship in
the legacy tree but declare **no** `@font-face` and are referenced by no
token, so they are deliberately not vendored. The Artisan Neo stylistic
variants (`fonts/artisan-neo/variants/*`) are likewise unreferenced and
excluded. JetBrains Mono per-weight statics (`100.woff2` … `800.woff2`)
duplicate the variable file's range and are excluded.

| File (under `modules/assets/fonts/`) | Legacy source (`…/assets/fonts/`) | Conversion | Family | Weights | License |
| --- | --- | --- | --- | --- | --- |
| `artisan-neo-variable.ttf` (879868 B, `sha256:028839c3…497c7678`) | `artisan-neo/artisan-neo-variable.ttf` | none (vendored bit-identical) | `"Artisan Neo"` | 100–900 | `licenses/artisan-neo-OFL.txt` (OFL-1.1, Inter-derived) |
| `cal-sans-variable.ttf` (773004 B, `sha256:454eb1b1…748be199`) | `calsans/variable.woff2` | `fonttools ttLib.woff2 decompress` (fonttools 4.63.0) | `"Cal Sans"` | 100–1000 | `licenses/cal-sans-OFL.txt` (OFL-1.1) |
| `jetbrains-mono-variable.ttf` (299920 B, `sha256:bea2565b…89384c2e`) | `jetbrains-mono/variable.woff2` | `fonttools ttLib.woff2 decompress` (fonttools 4.63.0) | `"JetBrains Mono"` | 100–800 | `licenses/jetbrains-mono-OFL.txt` (OFL-1.1) |
| `sigurd-artisan.ttf` (35216 B, `sha256:f33a95c3…fdb1288`) | `sigurd/sigurd-artisan.woff2` | `fonttools ttLib.woff2 decompress` (fonttools 4.63.0) | `"Sigurd Variable"` (CSS alias; file-internal name `"Sigurd Variable Light"`, see below) | 300–900 | `licenses/sigurd-artisan-NOTES.md` (no in-tree grant; attestation required) |

Full digests:

- `artisan-neo-variable.ttf`: `028839c365c896cd7202fd100157a293b231b21aaa049e28a6d81ec497c7678d`
- `cal-sans-variable.ttf`: `454eb1b1e066245cc4cf1b33507e52304b7257eaa07129883b47b748be199cd`
- `jetbrains-mono-variable.ttf`: `bea2565b91b6ca7b9e6afe607bd725f9f5aecb28abb3cea7da1dc89384c2ec2e`
- `sigurd-artisan.ttf`: `f33a95c3c81cdc9c0d1646b437b5bae4b97115d61cada224c47deaab1fdb1288`

## Family-name verification (fonttools, before vendoring)

Each TTF opens in `fontTools.ttLib.TTFont` and its `name` table reports the
vendored family (nameIDs 1/16): `artisan-neo-variable.ttf` → `"Artisan Neo"`;
`cal-sans-variable.ttf` → `"Cal Sans"`; `jetbrains-mono-variable.ttf` →
`"JetBrains Mono"`. The Sigurd file reports `"Sigurd Variable Light"` —
the CSS `font-family: "Sigurd Variable"` is an author-defined alias (as in
browsers), but DirectWrite matches the internal name table
(`IDWriteFontSet::GetMatchingFonts`, exact family match), so the file
registers under `"Sigurd Variable Light"` and the `"Sigurd Variable"`
wordmark role falls back until a follow-up aligns the role string. The
catalog keeps the CSS alias (the `typography_gradient` suite pins
catalog↔theme identity); this note records the divergence honestly.

## Native registration

Consumers never read these files from disk: `src/fonts.rs` embeds them with
`include_bytes!` and exposes `bundled_fonts()` (`Vec<Cow<'static, [u8]>>`)
for `gpui::TextSystem::add_fonts`, which feeds the platform loader
(DirectWrite in-memory font-file references on Windows,
`gpui-0.2.2/src/platform/windows/direct_write.rs:284–322`). The call belongs
at app startup, first inside `Application::new().run(|cx| …)` (see the
`register_bundled_fonts` helper in `artisan-ui`, which the orchestrator wires
into `native_application.rs`). Family names and weight ranges here must stay
identical to `artisan_ui::theme::TypographyTokens`; the
`typography_gradient` suite pins both sides.
