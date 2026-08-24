# SVG asset inventory — legacy frontend

Status: complete census of every way `modules/frontend` (the deprecated SvelteKit
frontend) produces or references SVG, with the disposition of each use for the
native port. The following Phase 6 stack entries add the companion
`modules/assets/manifest.toml` machine-readable records,
`modules/assets/svg/**` vendored sources, and typed API in
`modules/assets/src/lib.rs`; this entry includes the provenance and
license/attribution material under `modules/assets/licenses/**`.

Method: every count below was produced by a repeatable query against this
repository at stack-top `0cfb0a0`, then resolved through the import graph.
Package-provenance evidence was captured read-only from the pinned installs in
the sibling checkout `C:\Users\sander\Desktop\artisan-editor`
(`@tabler/icons-svelte@3.44.0`, `@selemondev/svgl-svelte@2.17.0`, junctions into
its pnpm store) and bound to `pnpm-lock.yaml` integrity entries. One dated
snapshot of the official `api.svgl.app` metadata for the 12 used logos was
captured as `licenses/svgl-api-evidence.json`; that API exposes attribution URLs
but no per-logo SPDX grant. Vendoring and validation require no network, Node
runtime, or browser.

## 1. Totals

| Channel | Distinct uses |
| --- | --- |
| Tabler direct imports (`@tabler/icons-svelte/icons/<slug>`) | 70 slugs / 130 import lines across 57 compiled source files after excluding the stray `+page.sv` duplicate (raw lexical scan: 132 lines / 58 files) |
| Tabler barrel imports (`import { IconX } from '@tabler/icons-svelte'`) | 8 symbols / 17 import lines across 15 files in 8 wrapper directories; adds exactly `chevron-up`, `minus` |
| **Tabler union (vendored)** | **72** |
| SVGL component imports | 12 components across 2 modules |
| Checked-in `.svg` files under `src/lib/assets/` | 13 (11 referenced → vendored byte-identical; 2 unreferenced → excluded, §7) |
| Inline static SVG components (Simple Icons marks, Lobe marks) | 8 |
| Inline static SVG inside product components (onboarding success check) | 1 |
| Dormant inline SVG component | 1 (`routes/components/opencode-icon.svelte`, zero consumers) |
| Data-driven SVG generators/widgets | 5 (§6) |
| Runtime-rendered SVG pipeline (Mermaid) | 1 (renderer-deferred, §5) |
| CSS/data-URL SVG payloads | nested base64 `<image>` inside the two app icons only; favicon is empty `data:,` (§8) |
| Packaging resources reaching into `src/lib/assets/**` | PNG/ICO only — no SVG packaging resource exists (§9) |
| **Vendored assets** | **104 files / 104 manifest entries** |

Reachability axis: production builds stub five development-only surfaces
(`modules/frontend/vite.config.ts` `development_only_surfaces()`):
`routes/debug/emulator`, `routes/debug/overlay`, `routes/debug/components`,
the whole `routes/drafts/starting` subtree (stubbing the page unroots its
variants/pieces), and `lib/conversation/emulator-scripts.ts`. Uses that live only
behind those surfaces are marked `dev-only`. Dev-only does not downgrade
disposition: PLAN requires vendoring every reference regardless of the first
native workflow.

Hygiene note: `routes/debug/overlay/+page.sv` is a stray duplicate of
`+page.svelte` with an extension nothing compiles. It is excluded from all
counts; it duplicates imports that the real route already contributes.

## 2. Tabler icons (72)

Extraction source: `dist/icons/<slug>.svelte` plus the shared renderer
`dist/Icon.svelte` and defaults `dist/defaultAttributes.js` from the pinned
`@tabler/icons-svelte@3.44.0` backing store. Each icon file carries only an
`iconNode` element list; standalone SVG is reconstructed with the exact default
root attributes the renderer applies (outline: `viewBox="0 0 24 24" fill="none"
stroke="currentColor" stroke-width="2" stroke-linecap="round"
stroke-linejoin="round"`; filled: `fill="currentColor" stroke="none"`; both
24×24). Every `iconNode` element name and attribute value is copied exactly;
only standalone root markup and indentation are materialized. No path is
redrawn. Package identity:
name/version/license verified from the backing store `package.json`; lockfile
binding: `pnpm-lock.yaml` line 1939, integrity
`sha512-ZJJMCHoqpvb9hLVn9dU+pn8LCdX/e+mJ/fC+EUaJT5nHEm0+IW4aKKYkYQI+rFMzN8ivj36MnMC5TGFi4H6zew==`.
License: MIT (package).

Filled variants (from each file's `type="filled"`): `bolt-filled`,
`player-stop-filled`, `star-filled`. All others outline.

### 2.1 Direct-import use sites

| Asset | Reachability | Sites (under `modules/frontend/src/`) |
| --- | --- | --- |
| `tabler.alert-triangle` | shipped | routes/components/conversation-trace.svelte; routes/components/conversation-usage-interruption-card.svelte |
| `tabler.arrow-bar-to-down` | shipped | routes/components/thread-hover-rail.svelte |
| `tabler.arrow-right` | shipped | routes/components/onboarding/view.svelte |
| `tabler.arrow-up` | shipped | routes/components/composer/controls.svelte; routes/drafts/starting/pieces/composer.svelte |
| `tabler.arrows-horizontal` | shipped | routes/components/model-selector/policy-controls.svelte |
| `tabler.arrows-minimize` | shipped | routes/components/settings/compaction-model.svelte |
| `tabler.bell` | shipped | routes/components/settings/nav.svelte |
| `tabler.bolt-filled` | shipped | routes/components/model-selector/policy-controls.svelte |
| `tabler.bot-id` | shipped | routes/components/thread-agents.svelte |
| `tabler.brain` | shipped | routes/components/model-selector/policy-controls.svelte |
| `tabler.brand-visual-studio` | shipped | routes/components/conversation-changes-card.svelte |
| `tabler.bug` | shipped | routes/components/conversation-trace.svelte |
| `tabler.check` | shipped | lib/components/markdown/code-snippet.svelte; routes/components/conversation-approval.svelte; routes/components/project-selector.svelte; routes/drafts/starting/pieces/project-menu.svelte |
| `tabler.checklist` | shipped | routes/components/sidebar-identity.svelte |
| `tabler.chevron-down` | shipped | routes/components/thread-composer.svelte; routes/drafts/starting/pieces/composer.svelte; routes/drafts/starting/variants/token.svelte |
| `tabler.chevron-left` | dev-only | routes/debug/components/+page.svelte; routes/drafts/starting/+page.svelte; routes/drafts/starting/variants/deck.svelte |
| `tabler.chevron-right` | shipped | routes/components/conversation-trace.svelte; routes/components/conversation-work-session.svelte; routes/components/model-selector/model-list.svelte; routes/components/workspace-file-tree.svelte; plus dev-only drafts/debug sites |
| `tabler.circle-check` | shipped | routes/components/conversation-approval.svelte; routes/components/onboarding/view.svelte |
| `tabler.circle-x` | shipped | routes/components/conversation-approval.svelte; routes/components/conversation-error-card.svelte; routes/components/conversation-trace.svelte |
| `tabler.code` | shipped | routes/components/sectioned-panel.svelte |
| `tabler.copy` | shipped | lib/components/markdown/code-snippet.svelte; routes/components/conversation-error-card.svelte; routes/components/conversation-turn-footer.svelte; routes/debug/emulator/+page.svelte (dev-only site) |
| `tabler.corner-down-left` | dev-only | routes/drafts/starting/variants/launcher.svelte |
| `tabler.device-laptop` | shipped | routes/components/thread-environment-card.svelte |
| `tabler.download` | shipped | routes/components/onboarding/view.svelte |
| `tabler.edit` | shipped | routes/components/command-menu.svelte |
| `tabler.file-diff` | shipped | routes/components/conversation-approval.svelte; routes/components/thread-environment-card.svelte |
| `tabler.file-off` | shipped | routes/components/editor-route.svelte |
| `tabler.file-pencil` | shipped | routes/components/conversation-trace.svelte |
| `tabler.file-search` | shipped | routes/components/conversation-trace.svelte |
| `tabler.file-text` | shipped | routes/components/conversation-trace.svelte |
| `tabler.file-x` | shipped | routes/components/conversation-trace.svelte |
| `tabler.folder` | shipped | routes/components/project-identity-mark.svelte; routes/components/thread-hover-rail.svelte; routes/components/workspace-file-tree.svelte; routes/components/workspace-header.svelte; routes/drafts/starting/variants/attach.svelte |
| `tabler.folder-code` | shipped | routes/components/thread-environment-card.svelte; routes/components/workspace-header.svelte |
| `tabler.folder-open` | shipped | routes/components/workspace-file-tree.svelte |
| `tabler.folder-plus` | shipped | routes/components/project-selector.svelte; plus eight dev-only draft variant/piece sites |
| `tabler.git-branch` | shipped | routes/components/thread-environment-card.svelte; routes/components/workspace-header.svelte; plus drafts sites (repo-state.svelte, variants/attach.svelte) |
| `tabler.list-details` | shipped | routes/components/conversation-trace.svelte |
| `tabler.loader-2` | shipped | routes/components/onboarding/view.svelte |
| `tabler.lock` | shipped | routes/components/model-selector/policy-controls.svelte |
| `tabler.login` | shipped | routes/components/onboarding/view.svelte |
| `tabler.maximize` | shipped | lib/components/markdown/mermaid-renderer.svelte |
| `tabler.message-circle` | shipped | routes/components/command-menu.svelte; routes/components/sectioned-panel.svelte |
| `tabler.message-plus` | shipped | routes/components/composer/controls.svelte |
| `tabler.messages` | shipped | routes/components/settings/nav.svelte |
| `tabler.palette` | shipped | routes/components/settings/nav.svelte |
| `tabler.pencil` | shipped | routes/components/composer/steering-lip.svelte |
| `tabler.player-play` | shipped | routes/components/forge-connection-overlay.svelte |
| `tabler.player-stop-filled` | shipped | routes/components/composer/controls.svelte |
| `tabler.player-track-next` | dev-only | routes/debug/emulator/+page.svelte; routes/debug/overlay/+page.svelte |
| `tabler.player-track-prev` | dev-only | routes/debug/emulator/+page.svelte; routes/debug/overlay/+page.svelte |
| `tabler.question-mark` | shipped | lib/engine/presentation.ts (`unknown_engine_mark` fallback glyph) |
| `tabler.refresh` | shipped | routes/components/conversation-usage-interruption-card.svelte; routes/components/conversation-work-session.svelte; routes/components/forge-connection-overlay.svelte; routes/components/settings/engine.svelte |
| `tabler.rotate-clockwise` | shipped | routes/components/settings/appearance.svelte |
| `tabler.search` | dev-only | routes/drafts/starting/variants/launcher.svelte |
| `tabler.selector` | shipped | lib/components/ui/native-select/native-select.svelte; routes/components/model-selector/view.svelte; routes/components/project-selector.svelte; routes/components/settings/compaction-model.svelte; routes/components/settings/font-picker.svelte; routes/components/thread-environment-card.svelte |
| `tabler.settings` | shipped | routes/components/command-menu.svelte; routes/components/sidebar-identity.svelte |
| `tabler.shield-lock` | shipped | routes/components/settings/nav.svelte |
| `tabler.shopping-bag` | shipped | routes/components/sectioned-panel.svelte |
| `tabler.sparkles` | shipped | routes/components/settings/nav.svelte |
| `tabler.star` | shipped | routes/components/model-selector/model-list.svelte |
| `tabler.star-filled` | shipped | routes/components/settings/models.svelte |
| `tabler.terminal-2` | shipped | routes/components/conversation-approval.svelte; routes/components/conversation-trace.svelte; routes/components/thread-terminals.svelte |
| `tabler.test-pipe` | shipped | routes/components/onboarding/view.svelte |
| `tabler.tool` | shipped | routes/components/conversation-trace.svelte |
| `tabler.trash` | shipped | routes/components/composer/steering-lip.svelte |
| `tabler.world` | shipped | lib/components/markdown/anchor.svelte |
| `tabler.world-search` | shipped | routes/components/conversation-trace.svelte |
| `tabler.x` | shipped | lib/components/markdown/mermaid-renderer.svelte; routes/components/composer/attachment-tray.svelte; routes/components/conversation-approval.svelte; routes/components/forge-connection-overlay.svelte; routes/components/image-viewer.svelte; routes/components/settings/models.svelte; routes/drafts/starting/variants/launcher.svelte (dev-only site) |
| `tabler.zoom-in` | shipped | lib/components/markdown/mermaid-renderer.svelte |
| `tabler.zoom-out` | shipped | lib/components/markdown/mermaid-renderer.svelte |

### 2.2 Barrel-import use sites (UI wrappers)

All shipped. Symbols canonicalize to kebab slugs; `chevron-up` and `minus`
exist only through the barrel.

| Asset | Wrapper site |
| --- | --- |
| `tabler.chevron-up` | lib/components/ui/accordion/accordion-trigger.svelte; lib/components/ui/select/select-scroll-up-button.svelte |
| `tabler.minus` | lib/components/ui/dropdown-menu/dropdown-menu-checkbox-item.svelte |
| `tabler.check` | context-menu radio/checkbox items; dropdown-menu radio/checkbox items; select/select-item; command/command-item (adds wrapper sites to §2.1 rows) |
| `tabler.chevron-down` | accordion-trigger; select/select-scroll-down-button (adds wrapper sites) |
| `tabler.chevron-right` | context-menu/context-menu-sub-trigger; dropdown-menu/dropdown-menu-sub-trigger (adds wrapper sites) |
| `tabler.search` | command/command-input (wrapper site; slug otherwise dev-only via drafts launcher) |
| `tabler.selector` | select/select-trigger (adds wrapper site) |
| `tabler.x` | dialog/dialog-content; sheet/sheet-content (add wrapper sites) |

Note: `tabler.search` becomes shipped once the `command` wrapper's
`command-input.svelte` barrel import is counted — the direct-import scan alone
mislabels it dev-only. The manifest's use-site rows encode both sites.

## 3. SVGL logos (12)

Import sites:

- `lib/vcs/presentation.ts`: `SvglGitHubLogo`, `SvglGitLabLogo`, `SvglGitLogo`,
  `SvglMicrosoftAzureLogo` — consumed through `repository_marks`
  (`RepositoryHost`-keyed; unknown/absent hosts fall back to plain Git).
- `lib/engine/presentation.ts`: `SvglClaudeAILogo`, `SvglCursorLogo`,
  `SvglDeepSeekLogo`, `SvglGeminiLogo`, `SvglGrokLogo`, `SvglMetaLogo`,
  `SvglOpenAILogo`, `SvglQwenLogo` — consumed through `engine_marks`
  (`claude/codex/cursor/grok/hermes/opencode2`) and `provider_marks`
  (17 provider ids; regex-based inference over raw model ids can select any
  provider mark even when the catalog lacks the model).

One component serves many ids (e.g. `SvglOpenAILogo` serves `codex`, `openai`,
`openai-codex`; `SvglGrokLogo` serves `grok` and `xai`). All shipped.

Extraction source: `dist/components/<Name>.svelte` template literal from the
pinned `@selemondev/svgl-svelte@2.17.0` backing store; the embedded upstream SVG
markup is preserved verbatim except removal of the component's `width=`/`height=`
props and `${restAttrs}` spread from the root tag (per-use sizing, not artwork;
viewBox geometry untouched). Package identity verified from backing store
`package.json`; lockfile binding: `pnpm-lock.yaml` line 1731, integrity
`sha512-0f4CXwZ2oIazs8hufSxP5Sp5HUhlPqFKNKx4AlqCulq4M5ZYDKJTwAjzcGktyFXFVSTzVMbsmqf5Po83y70CBA==`.

Per-logo licensing: the npm package license (MIT, © Selemondev) covers the
package code only and is **not** assumed to cover the logos. The upstream svgl
project publishes no per-logo SPDX field (`api.svgl.app` entries carry owner
site/brand URLs but no license attribute; snapshot checked into
`licenses/svgl-api-evidence.json`). Each logo therefore ships as the trademarked
brand mark of its owner with nominative-use attribution recorded per entry in
`manifest.toml` and summarized in `licenses/svgl-brand-marks-NOTES.md`. Where
svgl records an official brand-resource URL it is preserved as evidence
(GitHub <https://brand.github.com/>, Cursor <https://cursor.com/brand>,
Meta brand resources, OpenAI <https://openai.com/brand/>); the remaining logos
record their owner/product URL only. This is a recorded restriction, not a
resolved MIT grant.

## 4. Checked-in `.svg` files, brand wrappers, and inline static markup

Byte-preserving copies (verified by sha256 in the manifest):

| Asset | Legacy source | Consumers | Reachability |
| --- | --- | --- | --- |
| `artisan.app-icon` | src/lib/assets/barekey/artisan-app-icon.svg | lib/notifications/web-presenter.ts (system-notification icon) | shipped |
| `artisan.star` | src/lib/assets/barekey/artisan-star.svg | routes/components/settings/compaction-model.svelte (CSS luminance mask) | shipped |
| `artisan.logo-gradient` | src/lib/assets/barekey/logo-gradient.svg | routes/components/sectioned-panel.svelte (`--artisan-logo-gradient` background); routes/debug/applogo/+page.svelte (dev-only page) | shipped |
| `brands.hermes` | src/lib/assets/brands/hermes/logo.svg | brands/hermes/logo.svelte (`<img>` wrapper) → engine presentation `hermes` | shipped |
| `brands.kimi` | src/lib/assets/brands/kimi/logo.svg | brands/kimi/logo.svelte → `moonshot` | shipped |
| `brands.opencode` | src/lib/assets/brands/opencode/logo.svg | brands/opencode/logo.svelte → `opencode2`, `opencode`, `opencode-go` | shipped |
| `brands.zai` | src/lib/assets/brands/zai/logo.svg | brands/zai/logo.svelte → `zai`, `zhipu` | shipped |
| `jetbrains.text` | src/lib/assets/jetbrains-file-icons/dark/text.svg | lib/conversation/file-icon.ts fallback icon (suffix→language lookup over @artisan/data associations) | shipped |
| `jetbrains.ts-test` | src/lib/assets/jetbrains-file-icons/dark/ts-test.svg | lib/conversation/file-icon.ts (`typescript-test`) | shipped |
| `jetbrains.typescript` | src/lib/assets/jetbrains-file-icons/dark/typescript.svg | lib/conversation/file-icon.ts (`typescript`) | shipped |
| `jetbrains.svelte` | src/lib/assets/jetbrains-file-icons/dark/svelte.svg | lib/conversation/file-icon.ts (`svelte`) | shipped |

Inline static markup extracted to standalone SVG (only Svelte spread artifacts
removed; attributes, path data, indentation structure preserved):

| Asset | Legacy source | Consumers | Notes |
| --- | --- | --- | --- |
| `simple-icons.bitbucket` | lib/vcs/marks/bitbucket.svelte | vcs presentation `repository_marks.bitbucket` | doc comment attests Simple Icons CC0 |
| `simple-icons.codeberg` | lib/vcs/marks/codeberg.svelte | `repository_marks.codeberg` | same |
| `simple-icons.gitea` | lib/vcs/marks/gitea.svelte | `repository_marks.gitea` | same |
| `simple-icons.sourcehut` | lib/vcs/marks/sourcehut.svelte | `repository_marks.sourcehut` | same |
| `lobe.minimax` | brands/minimax/logo.svelte | provider mark `minimax` | Lobe Icons MIT (LICENSE.lobe-icons.txt) |
| `lobe.nvidia` | brands/nvidia/logo.svelte | provider mark `nvidia` | same |
| `lobe.tencent` | brands/tencent/logo.svelte | provider mark `tencent` | same |
| `lobe.xiaomi` | brands/xiaomi/logo.svelte | provider mark `xiaomi` (MiMo glyph per in-file doc comment) | same |
| `artisan.success-check` | routes/components/onboarding/view.svelte (~line 262) | onboarding harness "installed" state | hybrid: static path drawn by `.t-success-check` CSS stroke-dash animation; vendor the path, animate natively |

## 5. Renderer-deferred: Mermaid (a renderer capability, not a shader)

`lib/components/markdown/mermaid-rendering.ts` renders diagram text to SVG at
runtime via `beautiful-mermaid` 1.1.3, structurally sanitizes the result
(htmlparser2 allow-list) before `{@html}` insertion, and
`mermaid-renderer.svelte` wraps it with Tabler zoom/maximize/close controls.
Nothing here is a static asset: the census vendors none of it. PLAN defers the
settled Mermaid/math renderer selection; the sanitizer contract is recorded as
behavior evidence for that future work. Classification: `renderer-deferred`.
No custom shader is involved anywhere in the Mermaid path, and no other
shader-dependent SVG effect exists in the legacy frontend — the negative is
asserted explicitly after scanning all `<svg`, `data:image/svg`, and
filter/CSS-mask sites.

## 6. Data-driven-native widgets (reimplement with GPUI drawing; never flattened)

| Site | Nature | Native obligation |
| --- | --- | --- |
| lib/identity/gradient-avatar.ts (`GradientAvatarSvg`, line ~127) | deterministic SVG string generator: FNV-1a seed → Bayer-4 dithered cell runs on the Tailwind oklch palette; consumed by routes/components/sidebar-identity.svelte | reproduce the exact algorithm and palette values in GPUI path drawing (color-exact match possible; module docs pin them) |
| routes/components/context-usage-ring.svelte (line 36) | arc from `percent`/`compaction_percent` props via computed `stroke-dasharray`; tone-mixed color mixing | GPUI stroked-arc primitive with the same tone mixing |
| lib/components/activity/vertical-calendar-activity-grid.svelte (line 252) | small inline progress ring, same dash technique | same primitive |
| lib/components/ui/fade-arc/fade-arc.svelte (line 21) | spinner: two arcs with per-instance `$props.id()` gradient uids + CSS rotation (`fade-arc-spin`), leading/trailing opacity fade; attribution: loading-ui.com FadeArc | draw arcs natively with animated opacity; per-instance paint ids disappear with DOM rendering |
| routes/drafts/starting/variants/orbit-spring.svelte (line 285) | orbit guide with internal gradient defs (`url(#dz-orbit-guide)`) | dev-only surface; record only |

(Composition-only consumer, no drawing of its own:
routes/components/context-usage-gauge.svelte wraps the ring component.)

## 7. Excluded from vendoring (genuinely unreferenced; evidence recorded)

Repo-wide searches (`rg -i 'forge\.svg|artisan-forge-icon|opencode-icon'` across
all modules, no node_modules) return zero references outside this document:

| File | Evidence |
| --- | --- |
| src/lib/assets/barekey/forge.svg | no code/config/build reference anywhere in the repository; not a packaging resource (§9 edges are PNG/ICO only). Third-party tool credit preserved here: in-file comment "SVG created with Arrow, by QuiverAI (https://quiver.ai)" |
| src/lib/assets/barekey/artisan-forge-icon.svg | no reference anywhere; not a packaging resource |
| routes/components/opencode-icon.svelte | hand-drawn OpenCode glyph; zero consumers (superseded by `brands/opencode/logo.svelte`); dormant duplicate |

None is a packaging resource: the cross-module sweep found
`modules/broker/build.rs` embedding
`barekey/runtime-app-icons/foreground-gradient-symbol.ico`, and
`modules/desktop/src/app-icon.ts` resolving packaged filenames
`foreground-gradient-symbol.{png,ico}` / `plastic-jaw-shading.{png,ico}` — all
PNG/ICO, no SVG. Exclusion is reversible if product scope later wants these.

## 8. Raw / data-URL SVG findings

- Nested `data:image/svg+xml;base64` payloads exist only as `<image href>`
  elements *inside* `artisan-app-icon.svg` and `artisan-forge-icon.svg` (a full
  720×720 gradient backdrop serialized into each icon). The referenced
  `artisan-app-icon.svg` is vendored byte-for-byte and its nested payload is
  covered by the asset digest. The unreferenced `artisan-forge-icon.svg` remains
  excluded under §7, so its payload is legacy evidence rather than a manifest
  entry.
- `app.html` favicon is the deliberately empty `data:,` — there is no SVG
  favicon; nothing to vendor.
- CSP (`img-src 'self' data: blob:`) admits data/blob images at runtime; remote
  SVG cannot pass CSP. `{@html}` SVG enters only through the sanitized Mermaid
  path (§5).
- Pure-CSS masks/gradients with no asset URLs (utilities.css fade edges,
  shimmer, hatch, radial glows; active-thread-light; drafts.css) are styling,
  not SVG payload. The `docs-sidebar-logo-mark` utility consumes
  `--docs-sidebar-logo-cutout`, which is never assigned anywhere — dormant CSS,
  recorded so it is not mistaken for an asset consumer.

## 9. Packaging resources (non-SVG, retained in place)

| Edge | Consumer | Resource |
| --- | --- | --- |
| modules/broker/build.rs | Windows executable icon embed (build-time reach into the frontend asset tree) | barekey/runtime-app-icons/foreground-gradient-symbol.ico |
| modules/desktop/src/app-icon.ts | packaged runtime app-icon materialization from `packaged_root` | foreground-gradient-symbol.{png,ico}, plastic-jaw-shading.{png,ico} |
| routes/components/settings/appearance.svelte | preference previews | the two PNGs above |
| static/barekey-logo.png via string-keyed map | routes/debug/components/component-preview.svelte `image_sources["gallery-artisan-mark"]` | static/barekey-logo.png |

Consequence: any future cleanup of `src/lib/assets/**` must keep
`runtime-app-icons/*`; the broker build breaks silently otherwise.

## 10. Monochrome derivation and call-site flag semantics

`manifest.toml` records artwork paint: `monochrome = true` iff the distinct
literal paints among `fill`/`stroke`/`stop-color` attributes and style-block
declarations (excluding `none`, `currentColor`, `inherit`, `url(...)`; ignoring
subtrees of `<mask>`/`<clipPath>`) total ≤ 1, with no `<image>` and no gradient
element present. The Bazel test re-derives and compares.

Legacy call sites record a *different* predicate: `EngineMark.monochrome` /
`RepositoryMark.monochrome` mean "single-color logo that must invert with the
theme" (rendering policy). Artwork-mono therefore matches the call-site flag for
github/gitlab/azure/openai/cursor/grok/hermes/kimi/opencode/zai, and diverges —
correctly — where colored or chip-whitened marks are policy-flagged false while
their artwork is single-paint: `simple-icons.*` and `lobe.*` (currentColor marks
always whitened on brand-colored chips), `svgl.claude-ai` (#d97757),
`svgl.deepseek` (#4d6bfe), `svgl.qwen`/`svgl.openai` (currentColor/no-fill
artwork), and `tabler.search`'s slug-level dev-only/shipped nuance (§2.2).
Divergences are recorded per entry in the manifest notes rather than reconciled
away.

## 11. Licensing position by family

| Family | License evidence | Bound to |
| --- | --- | --- |
| tabler (72) | MIT; package `package.json` identity block captured | licenses/tabler-MIT.txt |
| svgl (12) | package code MIT (licenses/svgl-svelte-LICENSE.txt); logos are owner trademarks, per-logo SPDX not attestable from package or svgl metadata — recorded restriction with captured API evidence | licenses/svgl-brand-marks-NOTES.md, licenses/svgl-api-evidence.json |
| simple-icons (4) | CC0-1.0 per in-repo doc comments ("vendored from Simple Icons (CC0)") | licenses/simple-icons-CC0.txt (full text) |
| lobe (4) | MIT © 2023 LobeHub, directory-level attestation copied from the legacy tree | licenses/lobe-icons-MIT.txt |
| jetbrains (text/ts-test/typescript) | Apache-2.0, © 2000–2024 JetBrains s.r.o.; headers retained in each file; README attests curated subset of ziishaned/zed-jetbrains-icons | licenses/jetbrains-file-icons-README.md |
| jetbrains.svelte (dual provenance) | artwork sourced from MIT-licensed svgl-svelte package per the jetbrains README; the Svelte mark remains a brand asset of Svelte | licenses/svgl-svelte-LICENSE.txt + note |
| artisan (4) | first-party product artwork (720×720 app icon, star, gradient, success-check path); origin local, needs human provenance attestation (no in-tree statement exists today) | licenses/artisan-first-party-NOTES.md |
| brands (hermes/kimi/opencode/zai) | checked-in legacy brand marks without in-tree license statements; treated like svgl logos: owner trademark/attribution, needs_review=true | licenses/svgl-brand-marks-NOTES.md (same policy section) |

`anyhow` prohibition: unaffected; no first-party asset code or validation code
declares or imports `anyhow`.

## 12. Machine-readable records and validation

- The following asset-foundation stack entries provide
  `modules/assets/manifest.toml` — schema v1: one `[[asset]]` row per vendored
  file (stable id, family, name, origin kind/package/version/path, license SPDX +
  license file, viewBox, monochrome flag, normalized relative `source_path`,
  sha256, optional notes) and one `[[use]]` row per use site (id, legacy site
  path, channel, reachability, classification, linked asset(s)).
- `modules/assets/src/lib.rs` — typed `AssetId` lookup API exposing embedded SVG
  source and metadata; no npm/Svelte/browser/filesystem access at runtime.
- `tests/assets/assets_validation.rs` — hermetic Bazel `rust_test` proving
  manifest↔API set equality, ID uniqueness and grammar, normalized paths, XML/SVG
  root and viewBox well-formedness, recorded sha256 vs recomputed digest,
  monochrome re-derivation, license-file references, use-site closure (every
  static-vendored use resolves to a manifest entry and every entry is used), and
  representative metadata fixtures.

Commands (Bazel authoritative):

```text
bazel test //tests/assets:assets_validation_test
bazel build //modules/assets:assets
cargo check -p artisan-assets --locked --offline   # supplement only
```
