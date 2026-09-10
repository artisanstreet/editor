# Bundled native fonts

Spline Sans serves UI, headings and body. Spline Sans Mono serves code.
Artisan Neo serves the native titlebar wordmark at its single SemiBold
weight. Cal Sans remains vendored but is not currently requested by any
native surface. Spline faces are licensed under SIL OFL 1.1; Cal Sans is
licensed under SIL OFL 1.1; the Artisan Neo license record is the verbatim
in-repo frontend OFL file (see provenance below).

| File | Family | Bytes | SHA-256 |
| --- | --- | --- | --- |
| `artisan-neo-variable.ttf` (frontend source, not vendored natively) | Artisan Neo | 879868 | `028839c365c896cd7202fd100157a293b231b21aaa049e28a6d81ec497c7678d` |
| `artisan-neo-600.ttf` | Artisan Neo | 343516 | `7bf33c71b74f966e1af3cd258994ece37886272c6635930244afb2978b37b04f` |
| `cal-sans-700.ttf` | Cal Sans | 219644 | `0f760561909498736698f00a75622103356fa2e7481056754f77c6c4653a66c3` |
| `spline-sans-variable.ttf` | Spline Sans | 146896 | `65250939de0ab412d2b2349c9cb85304a5919fc3c5d4e74713a2d8a1f45f2947` |
| `spline-sans-mono-variable.ttf` | Spline Sans Mono | 118744 | `e20c1df32aa2f886e828cfcc81ca5c405d3f3b160990f6b892923e2e449bf525` |

Sources: Artisan Neo via the in-repo frontend vendoring
(`modules/frontend/src/lib/assets/fonts/artisan-neo/artisan-neo-variable.ttf`;
`@font-face` declares wght 100-900 per `modules/frontend/src/lib/styles/fonts.css`),
[Cal Sans](https://github.com/calcom/font) via the in-repo frontend vendoring
(`modules/frontend/src/lib/assets/fonts/calsans/calsans-bold.ttf`, vendored in
`3c0bf4ac`; frontend dependency `@fontsource/cal-sans 5.2.3` per
`modules/frontend/package.json`),
[Spline Sans](https://github.com/google/fonts/tree/main/ofl/splinesans),
[Spline Sans Mono](https://github.com/google/fonts/tree/main/ofl/splinesansmono).
Retrieved 2026-09-05 (Spline; Cal Sans and Artisan Neo provenance is the
in-repo frontend copy).
Corresponding OFL files are in `../licenses/`. Internal family names and fvar axes verified with fontTools 4.63.0.
Artisan Neo license note: `../licenses/artisan-neo-OFL.txt` is the byte-identical
copy of the in-repo `modules/frontend/static/fonts/artisan-neo-OFL.txt`; its
copyright header names "The Inter Project Authors (https://github.com/rsms/inter)"
and was transcribed verbatim without alteration. Upstream attribution for the
Artisan Neo design itself is unresolved here and left for root to resolve; no
copyright line was invented for this packet.
Static TrueType instances are embedded and registered before the first native window; no runtime font download or system installation is needed.

## Static weight instances for the native renderer

The active WGPU backend uses Cosmic Text. Its face matcher reads OS/2 weight metadata and does not instantiate the variable wght axis; registering only the source variable TTF selected Regular for every requested weight.

Registration therefore uses static instances at 300, 400, 500, 600 and 700 for each Spline family, plus the conversation-prose instances at 410 (body) and 630 (headings) for Spline Sans. Source variable files remain as provenance inputs, not registered faces. Generated with fontTools 4.63.0 `instantiateVariableFont(font, {"wght": weight}, inplace=True)`, with typographic family/style names and OS/2/head style-linking metadata set for each weight. The 410 face mirrors the 400 naming (`Spline Sans` / `Regular`, OS/2 410, REGULAR and USE_TYPO_METRICS bits); the 630 face mirrors the 600 naming (`Spline Sans SemiBold` / `SemiBold`, OS/2 630, REGULAR bit cleared with USE_TYPO_METRICS kept). Outlines come directly from the original variation; no synthetic emboldening is used, and no 400/600 binary was relabeled: the 410/630 outlines hash differently from both neighbors. Cal Sans ships a single static Bold (700) face: internal name-table family `Cal Sans`, subfamily `Bold`, OS/2 `usWeightClass` 700, no fvar axis. The native copy is byte-identical to the in-repo frontend `calsans-bold.ttf` (the sibling `variable.woff2` is WOFF2 and must never be vendored natively: DirectWrite's in-memory loader rejects it with `DWRITE_E_FILEFORMAT`). Artisan Neo ships a single static SemiBold (600) face generated with fontTools 4.63.0 `instantiateVariableFont(font, {"wght": 600, "opsz": 14}, inplace=True)` — opsz pinned at its axis default so no fvar axis remains — then named `Artisan Neo SemiBold` / `Regular` / `Artisan Neo SemiBold` / `ArtisanNeo-SemiBold` (nameIDs 1/2/4/6) with typographic family `Artisan Neo` / subfamily `SemiBold` (nameIDs 16/17), OS/2 `usWeightClass` 600 with the REGULAR bit cleared (USE_TYPO_METRICS kept), and head macStyle 0, mirroring the Spline 600 statics. Source fvar axes are opsz 14-32 (default 14) and wght 100-900 (default 400); internal family `Artisan Neo`, vendor ID `ARTS`. Outlines come directly from the original variation; no synthetic weight.

| Static face | Weight | SHA-256 |
| --- | --- | --- |
| `artisan-neo-600.ttf` | 600 | `7bf33c71b74f966e1af3cd258994ece37886272c6635930244afb2978b37b04f` |
| `cal-sans-700.ttf` | 700 | `0f760561909498736698f00a75622103356fa2e7481056754f77c6c4653a66c3` |
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

Twemoji Mozilla is registered as a color emoji fallback through the same catalog. Body and code family selections remain Spline. See [TWEMOJI.md](TWEMOJI.md) for the unmodified Mozilla v0.7.0 font, SHA-256, measured coverage, and licenses. Platform precedence is verified in the shared text backend tests.
