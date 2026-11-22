# Bundled native fonts

Spline Sans serves UI, headings and body. Spline Sans Mono serves code.
Spline faces are licensed under SIL OFL 1.1.

| File | Family | Bytes | SHA-256 |
| --- | --- | --- | --- |
| `spline-sans-variable.ttf` | Spline Sans | 146896 | `65250939de0ab412d2b2349c9cb85304a5919fc3c5d4e74713a2d8a1f45f2947` |
| `spline-sans-mono-variable.ttf` | Spline Sans Mono | 118744 | `e20c1df32aa2f886e828cfcc81ca5c405d3f3b160990f6b892923e2e449bf525` |

Sources:
[Spline Sans](https://github.com/google/fonts/tree/main/ofl/splinesans),
[Spline Sans Mono](https://github.com/google/fonts/tree/main/ofl/splinesansmono).
Retrieved 2026-09-05.
Corresponding OFL files are in `../licenses/`. Internal family names and fvar axes verified with fontTools 4.63.0.
Static TrueType instances are embedded and registered before the first native window; no runtime font download or system installation is needed.

## Static weight instances

The Editor embeds only static instances (`editor_assets::fonts::FACES`) and registers them with Aqua's shared font manager (`aqua_text::font::SharedFonts`) in `editor_ui::fonts::register_bundled_fonts`. The variable source files stay in this folder as provenance inputs; they are not embedded or registered.

The static instances are 300, 400, 500, 600 and 700 for each Spline family, plus the conversation-prose instances at 410 (body) and 630 (headings) for Spline Sans. Generated with fontTools 4.63.0 `instantiateVariableFont(font, {"wght": weight}, inplace=True)`, with typographic family/style names and OS/2/head style-linking metadata set for each weight. The 410 face mirrors the 400 naming (`Spline Sans` / `Regular`, OS/2 410, REGULAR and USE_TYPO_METRICS bits); the 630 face mirrors the 600 naming (`Spline Sans SemiBold` / `SemiBold`, OS/2 630, REGULAR bit cleared with USE_TYPO_METRICS kept). Outlines come directly from the original variation; no synthetic emboldening is used, and no 400/600 binary was relabeled: the 410/630 outlines hash differently from both neighbors.

| Static face | Weight | SHA-256 |
| --- | --- | --- |
| `spline-sans-300.ttf` | 300 | `3b6377266131248fa2f8b214c923d2053cdb308178e5b7159e613fbc0c6a24aa` |
| `spline-sans-400.ttf` | 400 | `7c30a74af85845ce794310814decc5ea533b9406afa5700ef13f3dd6824736f1` |
| `spline-sans-410.ttf` | 410 | `bb677b9bd6838888f3ca5729314815cbbe839c445e319383a5f32a150aa4c797` |
| `spline-sans-500.ttf` | 500 | `b9f3a3d544488b6cd0ffd91f9164cd3ea11579c5a17a02f2e0f1b4e2e040a491` |
| `spline-sans-600.ttf` | 600 | `6afafe275c3e0cba09386864e9b2a57da34306d72579c515a8e0e1fe4394c7d4` |
| `spline-sans-630.ttf` | 630 | `6be991ad420e182d84d0f2d8f67fe9e4eb09aa0e232edd4eb11eff91069b20899` |
| `spline-sans-700.ttf` | 700 | `dd077a732dcf04b0e02716e909089fb6f926afc0011d45bc4471ff439d2070eb` |
| `spline-sans-mono-300.ttf` | 300 | `4281c585363931c0b54bafe8ab5aff1facb8659c4f0a9b67656ad97060e16acd` |
| `spline-sans-mono-400.ttf` | 400 | `79de613980c1a21cc8ec3a82b1109fbbeaae103e90f3c1cdd48fb249fef0b707` |
| `spline-sans-mono-500.ttf` | 500 | `1c813d8df03beed3811e58dc1389da580e9fa8e81074a346a677ee9f33e28368` |
| `spline-sans-mono-600.ttf` | 600 | `8e193b99f84c2e2d106840fd794f1764c846894d3fb65b194f6f621153473984` |
| `spline-sans-mono-700.ttf` | 700 | `74189c4811dbf9abd88f2ad83950d7afcc9d230e7ecc092d36097efffa67090c` |

## Sigurd wordmark

`sigurd-400.ttf` is the static weight-400 instance of the dashboard's Sigurd
Variable web font that draws the sidebar wordmark. Size: 103876 bytes.
SHA-256: `3bd4098eb6fe40f576fd653683bc43fe8ad8fc70ab14a2ad1970ea64e6c288ed`.
It is embedded in every Editor build. See `../licenses/sigurd-NOTES.md` for
its provenance and licensing status, which is unconfirmed.
