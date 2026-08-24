# SVGL and brand-mark logos — licensing position

Scope: `modules/assets/svg/svgl/*` (12 entries) and
`modules/assets/svg/brands/*` (hermes, kimi, opencode, zai).

## What is attested

- The pinned npm package `@selemondev/svgl-svelte@2.17.0` is MIT
  (© Selemondev, `licenses/svgl-svelte-LICENSE.txt`). That license covers the
  package's wrapper code. **It is not treated as a license grant for the logo
  artwork.**
- The upstream svgl project (`pheralb/svgl`, svgl.app) publishes per-logo
  metadata without a license field; its API entries carry the owner/product URL
  and, where available, an official brand-resource URL. A snapshot of exactly
  the 12 vendored logos is checked in as extraction-time evidence:
  `licenses/svgl-api-evidence.json` (captured 2026-08-24).
- Per-logo SPDX identifiers are therefore **not attestable** from the installed
  package, its lockfile entry, or svgl's own metadata.

## Recorded restriction

Each svgl/brand asset in `manifest.toml` carries
`license_spdx = "LicenseRef-Brand-Mark"` with a per-logo note naming the owner.
This records precisely what is known: these are the trademarked brand marks of
their respective owners (GitHub, GitLab, Git, Microsoft Azure, Anthropic
Claude, Cursor, DeepSeek, Google Gemini, xAI Grok, Meta, OpenAI, Alibaba Qwen,
Nous Research Hermes, Moonshot AI Kimi, OpenCode, Z.ai). Use to identify the
corresponding product/service (nominative use) is how the legacy frontend used
them; nothing here claims a copyright license from the owner. Brand-resource
URLs published by svgl are preserved as attribution evidence.

`needs_review = true` is set on all of these entries so legal review before any
external distribution is explicit rather than assumed.

## Legacy brands directory

`src/lib/assets/brands/{hermes,kimi,opencode,zai}/logo.svg` were checked into
the legacy repository without license statements. `LICENSE.lobe-icons.txt`
attests Lobe Icons MIT for the four *inline* mark components only (minimax,
nvidia, tencent, xiaomi — each carrying an in-file doc comment), not for these
four files. They are recorded under the same brand-mark policy above;
`hermes/logo.svg` additionally embeds `<title>NousResearch</title>` and matches
Lobe Icons' Nous Research glyph, which is noted per-entry but not upgraded into
a license claim.
