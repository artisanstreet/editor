# Bundled typefaces — manifest

The four `@font-face` families the legacy editor actually declares
(`modules/frontend/src/lib/styles/fonts.css:7–37`) and spends through the
theme's `--font-*` tokens (`theme.css:313–315`, `fonts.css:39–42`) and
`typography_css_stacks` (`modules/frontend/src/lib/appearance/typography.ts`,
defaults `text: "Artisan Neo"`, `code: "JetBrains Mono"`). Bytes are
bit-identical to the legacy `@font-face` `src` files; lengths and SHA-256
digests below pin that identity.

Geist (`fonts/geist/*`) and Neue Montreal (`fonts/neue-montreal/*`) ship in
the legacy tree but declare **no** `@font-face` and are referenced by no
token, so they are deliberately not vendored. The Artisan Neo stylistic
variants (`fonts/artisan-neo/variants/*`) are likewise unreferenced and
excluded. JetBrains Mono per-weight statics (`100.woff2` … `800.woff2`)
duplicate the variable file's range and are excluded.

| File (under `modules/assets/fonts/`) | Legacy source (`…/assets/fonts/`) | Family | Weights | License |
| --- | --- | --- | --- | --- |
| `artisan-neo-variable.woff2` (354924 B, `sha256:3288f439…a695dd9`) | `artisan-neo/artisan-neo-variable.woff2` | `"Artisan Neo"` | 100–900 | `licenses/artisan-neo-OFL.txt` (OFL-1.1, Inter-derived) |
| `cal-sans-variable.woff2` (210520 B, `sha256:5edf7b1b…290a9105`) | `calsans/variable.woff2` | `"Cal Sans"` | 100–1000 | `licenses/cal-sans-OFL.txt` (OFL-1.1) |
| `jetbrains-mono-variable.woff2` (113672 B, `sha256:31ec365b…a42ca9ac0e`) | `jetbrains-mono/variable.woff2` | `"JetBrains Mono"` | 100–800 | `licenses/jetbrains-mono-OFL.txt` (OFL-1.1) |
| `sigurd-artisan.woff2` (18812 B, `sha256:fbfd5300…3341e722`) | `sigurd/sigurd-artisan.woff2` | `"Sigurd Variable"` | 300–900 | `licenses/sigurd-artisan-NOTES.md` (no in-tree grant; attestation required) |

Full digests:

- `artisan-neo-variable.woff2`: `3288f439ab906f906e54fd3dc395dd6b57550459025b73edf739887efa695dd9`
- `cal-sans-variable.woff2`: `5edf7b1b3cff38089e0f623c4d70b06ab4920a6dd3ed77fe143b8f0d290a9105`
- `jetbrains-mono-variable.woff2`: `31ec365b93e4bad6f202ce23352a56d01ca4462b2afc782ed2cf6fa42ca9ac0e`
- `sigurd-artisan.woff2`: `fbfd5300f137dc15cd9b048123fc09455878c12154d2e85cc7455efe3341e722`

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
