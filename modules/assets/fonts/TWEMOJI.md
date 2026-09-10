# Twemoji Mozilla COLR fallback face (provenance + license)

Fallback-only color-emoji face for sequences the platform font does not
cover. Never a primary text family: Spline Sans/Spline Sans Mono remain
the only text faces (user-selected; typography separately audited), and
this face participates exclusively through cosmic/fontdb fallback
resolution. Registration (`fonts.rs`/`FONTS.md`/manifest) is owned by
the prose worker — this document and the TTF are the complete packet
here. Shared integration contract:
`docs/reference-emoji-interface.md`.

## Source

- Official release asset `Twemoji.Mozilla.ttf` from
  `github.com/mozilla/twemoji-colr/releases/download/v0.7.0/`
  (tag `v0.7.0`, published 2022-10-28, Twemoji 14 repertoire).
- Size 1,474,284 bytes; SHA-256
  `6d90152ee0d29e82fe2a87793af5aa4b7ad13e6538360889e141e81ed299ee8e`.
- Retrieved 2026-09-10 over HTTPS; hash verified after copy into this
  tree. No modification, no subsetting, no re-encode.

## Measured coverage (fontTools read of the vendored bytes, not labels)

- PostScript nameID 6: `TwemojiMozilla` (allowlisted for the color
  raster path alongside the platform faces).
- COLR version 0 + CPAL; single GSUB `ccmp` lookup; 1,418 cmap entries,
  emoji-only (ASCII `a` absent — cannot shadow body text).
- Present: U+1F389, U+2764, U+FE0F, U+200D, regional indicators, skin
  tones, `#`, U+20E3, U+1F44D/U+1F468, plus precomposed ZWJ-sequence
  layer glyphs.
- Absent: `U+FE0E` (VS15 — text-presentation requests correctly fall
  through to the platform), U+1F6DC, U+1FAE8, and other post-14
  additions. Coverage is decided by shaping, never by version guess.

## License (verbatim from the `v0.7.0` tag `LICENSE.md`)

## License for the Code

Copyright 2016-2018, Mozilla Foundation

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.



## License for the Visual Design

The Emoji art in the twe-svg.zip archive comes from [Twemoji](https://twitter.github.io/twemoji),
and is used and redistributed under the CC-BY-4.0 [license terms](https://github.com/twitter/twemoji#license)
offered by the Twemoji project.

### Creative Commons Attribution 4.0 International (CC BY 4.0)
https://creativecommons.org/licenses/by/4.0/legalcode
or for the human readable summary: https://creativecommons.org/licenses/by/4.0/


#### You are free to:
**Share** - copy and redistribute the material in any medium or format

**Adapt** - remix, transform, and build upon the material for any purpose, even commercially.

The licensor cannot revoke these freedoms as long as you follow the license terms.


#### Under the following terms:
**Attribution** - You must give appropriate credit, provide a link to the license,
and indicate if changes were made.
You may do so in any reasonable manner, but not in any way that suggests the licensor endorses you or your use.

**No additional restrictions** - You may not apply legal terms or **technological measures**
that legally restrict others from doing anything the license permits.

#### Notices:
You do not have to comply with the license for elements of the material in the public domain
or where your use is permitted by an applicable exception or limitation. No warranties are given.
The license may not give you all of the permissions necessary for your intended use.
For example, other rights such as publicity, privacy, or moral rights may limit how you use the material.
