# Bundled native fonts

Spline Sans serves UI, headings and body. Spline Sans Mono serves code.
Cal Sans serves the native titlebar wordmark at its single Bold weight.
Spline faces are licensed under SIL OFL 1.1; Cal Sans is licensed under SIL
OFL 1.1.

| File | Family | Bytes | SHA-256 |
| --- | --- | --- | --- |
| `cal-sans-700.ttf` | Cal Sans | 219644 | `0f760561909498736698f00a75622103356fa2e7481056754f77c6c4653a66c3` |
| `spline-sans-variable.ttf` | Spline Sans | 146896 | `65250939de0ab412d2b2349c9cb85304a5919fc3c5d4e74713a2d8a1f45f2947` |
| `spline-sans-mono-variable.ttf` | Spline Sans Mono | 118744 | `e20c1df32aa2f886e828cfcc81ca5c405d3f3b160990f6b892923e2e449bf525` |

Sources: [Cal Sans](https://github.com/calcom/font) via the in-repo frontend
vendoring (`modules/frontend/src/lib/assets/fonts/calsans/calsans-bold.ttf`,
vendored in `3c0bf4ac`; frontend dependency `@fontsource/cal-sans 5.2.3` per
`modules/frontend/package.json`), [Spline Sans](https://github.com/google/fonts/tree/main/ofl/splinesans), [Spline Sans Mono](https://github.com/google/fonts/tree/main/ofl/splinesansmono). Retrieved 2026-09-05 (Spline; Cal Sans provenance is the in-repo frontend copy, byte-identical).
Corresponding OFL files are in `../licenses/`. Internal family names and fvar axes verified with fontTools 4.63.0.
Static TrueType instances are embedded and registered before the first native window; no runtime font download or system installation is needed.

## Static weight instances for the native renderer

The active WGPU backend uses Cosmic Text. Its face matcher reads OS/2 weight metadata and does not instantiate the variable wght axis; registering only the source variable TTF selected Regular for every requested weight.

Registration therefore uses static instances at 300, 400, 500, 600 and 700 for each Spline family. Source variable files remain as provenance inputs, not registered faces. Generated with fontTools 4.63.0 `instantiateVariableFont(font, {"wght": weight}, inplace=True)`, with typographic family/style names and OS/2/head style-linking metadata set for each weight. Outlines come directly from the original variation; no synthetic emboldening is used. Cal Sans ships a single static Bold (700) face: internal name-table family `Cal Sans`, subfamily `Bold`, OS/2 `usWeightClass` 700, no fvar axis. The native copy is byte-identical to the in-repo frontend `calsans-bold.ttf` (the sibling `variable.woff2` is WOFF2 and must never be vendored natively: DirectWrite's in-memory loader rejects it with `DWRITE_E_FILEFORMAT`).

| Static face | Weight | SHA-256 |
| --- | --- | --- |
| `cal-sans-700.ttf` | 700 | `0f760561909498736698f00a75622103356fa2e7481056754f77c6c4653a66c3` |
| `spline-sans-300.ttf` | 300 | `3b6377266131248fa2f8b214c923d2053cdb308178e5b7159e613fbc0c6a24aa` |
| `spline-sans-400.ttf` | 400 | `7c30a74af85845ce794310814decc5ea533b9406afa5700ef13f3dd6824736f1` |
| `spline-sans-500.ttf` | 500 | `b9f3a3d544488b6cd0ffd91f9164cd3ea11579c5a17a02f2e0f1b4e2e040a491` |
| `spline-sans-600.ttf` | 600 | `6afafe275c3e0cba09386864e9b2a57da34306d72579c515a8e0e1fe4394c7d4` |
| `spline-sans-700.ttf` | 700 | `dd077a732dcf04b0e02716e909089fb6f926afc0011d45bc4471ff439d2070eb` |
| `spline-sans-mono-300.ttf` | 300 | `4281c585363931c0b54bafe8ab5aff1facb8659c4f0a9b67656ad97060e16acd` |
| `spline-sans-mono-400.ttf` | 400 | `79de613980c1a21cc8ec3a82b1109fbbeaae103e90f3c1cdd48fb249fef0b707` |
| `spline-sans-mono-500.ttf` | 500 | `1c813d8df03beed3811e58dc1389da580e9fa8e81074a346a677ee9f33e28368` |
| `spline-sans-mono-600.ttf` | 600 | `8e193b99f84c2e2d106840fd794f1764c846894d3fb65b194f6f621153473984` |
| `spline-sans-mono-700.ttf` | 700 | `74189c4811dbf9abd88f2ad83950d7afcc9d230e7ecc092d36097efffa67090c` |
