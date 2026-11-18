# SVGL and brand-mark logos — licensing position

Scope: `crates/editor-assets/svg/svgl/*` (claude-ai, deepseek, git, github,
gitlab, meta, microsoft-azure, openai, qwen) and
`crates/editor-assets/svg/brands/*` (kimi, opencode, opencode-light,
reflection, zai).

## What is attested

- The pinned npm package `@selemondev/svgl-svelte@2.17.0` is MIT
  (© Selemondev, `licenses/svgl-svelte-LICENSE.txt`). That license covers the
  package's wrapper code. **It is not treated as a license grant for the logo
  artwork.**
- The upstream svgl project (`pheralb/svgl`, svgl.app) publishes per-logo
  metadata without a license field; its API entries carry the owner/product URL
  and, where available, an official brand-resource URL. A snapshot of the 12
  logos vendored at extraction time is checked in as evidence:
  `licenses/svgl-api-evidence.json` (captured 2026-08-24). Three of those
  logos have since been removed; the scope above is the current set.
- Per-logo SPDX identifiers are therefore **not attestable** from the installed
  package, its lockfile entry, or svgl's own metadata.

## Recorded restriction

Each svgl/brand asset is treated as `LicenseRef-Brand-Mark`.
This records precisely what is known: these are the trademarked brand marks of
their respective owners (GitHub, GitLab, Git, Microsoft Azure, Anthropic
Claude, DeepSeek, Meta, OpenAI, Alibaba Qwen, Moonshot AI Kimi, OpenCode,
Reflection AI, Z.ai). Use to identify the
corresponding product/service (nominative use) is how the legacy frontend used
them; nothing here claims a copyright license from the owner. Brand-resource
URLs published by svgl are preserved as attribution evidence.

No legal review of these marks is recorded. The asset catalog
(`crates/editor-assets/src/lib.rs`) carries no review flag and no release
check blocks on them; this note is the only record that permission from the
owners has not been confirmed.

## Legacy brands directory

`src/lib/assets/brands/{kimi,opencode,zai}/logo.svg` were checked into
the legacy repository without license statements. `LICENSE.lobe-icons.txt`
attests Lobe Icons MIT for the four *inline* mark components only (minimax,
nvidia, tencent, xiaomi - each carrying an in-file doc comment), not for these
three files. They are recorded under the same brand-mark policy above.

`opencode.svg` and `opencode-light.svg` were replaced on 2026-10-19 with
OpenCode's official dark- and light-mode marks (`OpenCode_light_dark.zip`),
cropped to the glyph with the square background removed.

## Model-lab marks (2026-10-20)

`brands/reflection.svg` is Reflection AI's mark as models.dev serves it
(`https://models.dev/logos/reflection.svg`; the models.dev repository is MIT,
the mark itself Reflection AI's). It identifies the lab of models in the model
menu, under the same `LicenseRef-Brand-Mark` position. `lobe/google.svg`,
`lobe/mistral.svg`, `lobe/stepfun.svg` and `lobe/xai.svg` come from
`@lobehub/icons-static-svg@1.95.1` (MIT, `lobe-icons-MIT.txt`).
