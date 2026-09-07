# Legacy UI Inventory — wrappers, Bits UI 2.18.1, and the selected first workflow

Phase 6 artifact (`docs/PLAN.md` §Phase 6, lines 584–633). It records what the legacy
SvelteKit frontend actually does: every local UI wrapper under
`modules/frontend/src/lib/components/ui/`, every direct `bits-ui` import, the reachability of
each, the style/token layer beneath them, and full behavior contracts for the selected
initial workflow. Companion artifacts `docs/ui/GPUI_CAPABILITIES.md` and `docs/ui/ASSETS.md`
(§8) own the native-capability audit and the SVG asset census respectively; their conclusions
are not asserted here.

## 0. Evidence identity

| Field | Value |
| --- | --- |
| Repository state inventoried | worktree `editor-agent-worktrees/phase-6-inventory`, commit `0cfb0a0c4b36f02c646af296cddf726d3d1058e4` ("docs: define phase-lead orchestration model"), working tree clean before this file |
| Legacy frontend source | `modules/frontend/src/**` in this worktree (read directly; every path below is repository-relative to the repo root) |
| Pinned dependency implementation | `bits-ui@2.18.1`, read at sibling install `C:\Users\sander\Desktop\artisan-editor\modules\frontend\node_modules\bits-ui` (SymbolicLink) → pnpm backing `C:\Users\sander\Desktop\artisan-editor\node_modules\.pnpm\bits-ui@2.18.1_@internation_f975dc757b1e1c647bc1ec6669701178\node_modules\bits-ui`; `package.json` confirms `"name": "bits-ui", "version": "2.18.1"` |
| Lockfile identity | `C:\Users\sander\Desktop\artisan-editor\pnpm-lock.yaml`: importer devDependency reference, snapshot key `bits-ui@2.18.1:` at line 2363, peer-resolved key `bits-ui@2.18.1(@internationalized/date@3.12.2)(…)(svelte@5.56.4)` at line 5821 |
| Abbreviation | `<bits>` below means the pnpm backing path above; cited Bit paths are `<bits>\dist\…` |
| Frontend pins relevant here | `modules/frontend/package.json`: `bits-ui 2.18.1` (:61), `shadcn-svelte 1.4.1` (devDependency), `tailwind-variants 3.2.2` (:71), `tailwindcss 4.3.2` (:72), `tw-animate-css 1.4.0` (:73), `mode-watcher 1.1.0` (:50), `svelte 5.56.4` (:66); `components.json`: style `maia`, baseColor `neutral`, iconLibrary `tabler` |
| Additional pinned style sources read | shadcn-svelte: sibling backing `C:\Users\sander\Desktop\artisan-editor\node_modules\.pnpm\shadcn-svelte@1.4.1_svelte@5.56.4\node_modules\shadcn-svelte\dist\tailwind.css`; tw-animate-css: sibling install `C:\Users\sander\Desktop\artisan-editor\modules\frontend\node_modules\tw-animate-css` (package.json version 1.4.0) → `dist\tw-animate.css`; compiled-output cross-check: sibling build artifact `modules\frontend\.svelte-kit\output\server\_app\immutable\assets\_layout.BaJXhTOH.css` (evidence in §5.10) |

All counts and classifications in §§1–5 were produced by queries executed in this worktree
during this session (queries reproduced inline). Sections 6–7 trace code that was read file-by-file.

## 1. Reachability methodology

**Graph definition.** Roots are every SvelteKit route file under `modules/frontend/src/routes`
(file-convention routing). Edges are static imports/exports, resolved through the aliases in
`modules/frontend/vite.config.ts` (`$lib` → `./src/lib`, `$` → `./src/routes`), wrapper barrels
(`index.ts`), member-path imports, namespace imports (`import * as X`), re-exports, and
type-only imports. Dynamic `import()` sites were enumerated and none touch
`components/ui` or `bits-ui` (query below; the 8 files with dynamic imports are CodeMirror
languages, Shiki grammars, Markdown render pipelines, Sentry, and orchestration).

**Production vs dev graphs.** `vite.config.ts:110–146` defines `development_only_surfaces()`,
activated only for production builds (`vite.config.ts:336`), replacing five modules with stubs:
`lib/conversation/emulator-scripts.ts`, `routes/debug/emulator/+page.svelte`,
`routes/debug/overlay/+page.svelte`, `routes/debug/components/+page.svelte`,
`routes/drafts/starting/+page.svelte`. Independently, every debug/drafts page also self-guards
at runtime with `import { dev } from "$app/environment"` (verified in all 7 files:
`routes/debug/{applogo,logo,onboarding,emulator,overlay,components}/+page.svelte`,
`routes/drafts/starting/+page.svelte`). Stubbing removes modules from the production bundle;
the `dev` guard changes rendering only. Both mechanisms are recorded per entry.

**Classification** (mutually exclusive):

- `reachable-production` — in the production closure via ≥1 non-stubbed root; concrete call site cited.
- `dev/gallery-only` — enters only through stubbed debug/draft roots. At wrapper level currently **empty**: the debug component gallery (`routes/debug/components/`) exercises production components via the `$` alias, and the drafts subtree imports no wrapper and no bits-ui. Gallery-only *call sites* exist inside otherwise-reachable wrappers (Button ×3, Select ×1 from debug pages) and are noted in consumer lists, not as wrapper classes.
- `wrapper-internal` — only consumers are other wrapper directories; **not terminal**: inherits its most-reachable consumer's class, with the inheritance chain recorded.
- `dormant` — zero import sites anywhere in `src` outside its own directory; confirmed by a second symbol-level negative-space pass.
- `packaging-only` — referenced only by build config, never by a graph node.

**Queries used (ripgrep over this worktree):**

```text
Q1 imports      rg -n 'from ["']bits-ui' modules/frontend/src -g '*.ts' -g '*.svelte'
Q2 consumers    rg -o 'components/ui/[a-z-]+' -g '*.ts' -g '*.svelte' -g '!**/components/ui/**' modules/frontend/src
Q3 composition  rg -o 'from "\$lib/components/ui/[a-z-]+' modules/frontend/src/lib/components/ui
Q4 dynamic      rg -n 'import\(' modules/frontend/src -g '*.ts' -g '*.svelte'
Q5 dormancy     rg -n '\b<Symbol>\b' modules/frontend/src -g '*.ts' -g '*.svelte'
```

**Validation performed:** Q2 results grouped and diffed against per-wrapper reading; Q5 run
for all six zero-consumer candidates; Q4 confirmed empty for the census surface; the stub
list re-read from `vite.config.ts` rather than trusted from reconnaissance notes; the two
product bypasses located directly. **Not performed:** a compiler-grade module-graph dump
(would require installing the JS toolchain; out of scope for this packet).

**Stray artifact:** `routes/debug/overlay/+page.sv` — an uncompiled near-duplicate of the
overlay debug page (wrong extension; nothing builds it). Excluded from all counts.

## 2. Wrapper census — all 32 directories

Aggregate, verified: 32 directories, **178 files** (146 `.svelte` + 32 `index.ts` barrels;
every directory has exactly one barrel and no other `.ts` file). All consumption flows
through barrels or explicit member paths; no bare-root form exists. Exactly **20 directories
import `bits-ui`** (107 import lines: 105 value imports shaped `import { X as XPrimitive }`,
2 type-only), and **12 are first-party**. Distinct Bits families consumed: the 20 matching
wrappers, plus helper `useId` (§3). Total direct import lines: **109** (107 wrapper-internal
+ 2 product bypasses).

Legend: **Backing** = B(its-backed)/F(irst-party). **Reach** = P(roduction-reachable),
D(ormant), W(wrapper-internal→inherited). **Behavior source** names who owns what the
consumer sees: `callsite` (product component), `wrapper` (this directory), `bits` (pinned
package), `css` (style layer §5), `tv` (`tailwind-variants` recipe in the wrapper).

| # | Directory | Backing | bits-ui lines | Reach | External consumers (distinct files, zone) | Wrapper members beyond barrel | Styling/variants | Behavior source | Disposition |
|---|-----------|---------|---------------|-------|--------------------------------------------|-------------------------------|------------------|-----------------|-------------|
| 1 | accordion | B | 4 | **Dormant** | none (Q5 `\bAccordion\b`: self only) | Root/Item/Trigger/Content(+Header) | rounded-2xl border; `animate-accordion-down/up` classes; chevron swap icons | wrapper + bits | Do not port; record only |
| 2 | alert | F | 0 | **Dormant** | none (barrel exports `Root as Alert`; product `alert` hits are `role="alert"` ARIA, verified at `routes/components/thread-route-gate.svelte:113`, `composer/action-failure.svelte:32`, `conversation-approval.svelte:122`, `forge-connection-overlay.svelte:270`) | Alert/Title/Description/Action | tv: default/destructive; border px-4 py-3 rounded-lg grid | wrapper (`tv`) | Product announces failures via `role="alert"` paragraphs instead — port that intent, not the wrapper |
| 3 | alert-dialog | B | 9 | **Dormant** | none (Q5 `\bAlertDialog\b`: self only) | Root/Trigger/Overlay/Content/Title/Description/Action/Cancel + Header/Footer/Media (no bits) | tv-free; content `rounded-4xl p-6 ring-1 max-w-xs sm:max-w-md`, centered fixed top-1/2 left-1/2; action/cancel reuse `buttonVariants` | wrapper + bits (alert variant: `role="alertdialog"`, content tabindex −1) | Do not port; confirmations can reuse modal primitive later |
| 4 | avatar | B | 3 | Reachable-P | 1 product: `routes/components/sidebar-identity.svelte` | Root/Image/Fallback | Root `size-8 rounded-full overflow-hidden`; fallback bg-muted text-xs | wrapper + bits (image load state) | Port minimal image-with-fallback behavior |
| 5 | badge | F | 0 | Reachable-P | 3 product: `conversation-change`, `conversation-prompt`, `settings/engine` | Badge | tv: default/secondary/destructive/outline/ghost/link; h-5 rounded-4xl text-xs; renders `span`/`a` | wrapper (`tv`) | Port as styled chip |
| 6 | button | F | 0 | Reachable-P | 22 files (19 product + 3 debug pages); deepest shared leaf, +6 internal consumers (§4) | Button (renders `<button>` or `<a href>`) | tv `buttonVariants`: variants default/outline/secondary/ghost/destructive/link; sizes default/xs/sm/lg/icon/icon-xs/icon-sm/icon-lg (h-9/h-6/h-8/h-10/size-9/size-6/size-8/size-10) | wrapper (`tv`) + callsite props | Port variant table verbatim; anchor-form disabled semantics matter (aria-disabled, tabindex −1, href dropped) |
| 7 | card | F | 0 | Reachable-P | 2 product: `conversation-change`, `conversation-prompt` | Card/Header/Title/Description/Content/Footer/Action | Root: `ring-foreground/10 bg-card rounded-2xl py-(--card-spacing)` `[--card-spacing:--spacing(6)]`, size sm → spacing 4; `@utility card` shadow (utilities.css:31) | wrapper + css | Port surface + spacing token |
| 8 | collapsible | B | 3 | Reachable-P | 1 product: `model-selector/model-list.svelte` (initial state `open`; trigger stays user-toggleable, :170–199) | Root/Trigger/Content | no wrapper classes; consumers style everything; call site plays accordion height keyframes on toggle (§6.6 item 3) | bits (open state) + callsite styling | Port as expand/collapse state primitive |
| 9 | command | B | 10 (incl. 1 type-only; +`useId`) | Reachable-P | 2 product: `routes/components/command-menu.svelte` (⌘K palette), `settings/font-picker.svelte` | Root/Dialog/Input/List/Group/GroupHeading(via Group)/Item/LinkItem/Empty/Separator/Shortcut/Loading | Root `bg-popover rounded-4xl p-1`; List `max-h-72 no-scrollbar overflow-y-auto`; Item `rounded-sm px-2 py-1.5 data-selected:bg-muted`, `in-data-[slot=dialog-content]:rounded-lg!`; Input composes InputGroup + IconSearch addon | bits (cmdk ranking/filtering) + wrapper + callsite | Port fuzzy-search menu as first-party; ranking algorithm is product-visible (§6.2) |
| 10 | context-menu | B | 14 | Reachable-P | 2 product: `thread-hover-rail.svelte` (`* as ContextMenu`, Root/Trigger/Content/Item at :590–640), `conversation-changes-card.svelte` | Root/Trigger/Content/Portal/Item/Group/GroupHeading/Label—/Separator/Sub* /Checkbox*/Radio* | Content identical recipe to dropdown-menu-content (min-w-48 rounded-2xl p-1 shadow-2xl ring-1 duration-100); Item destructive variant; Trigger `select-none` | bits (right-press open) + wrapper | Port context-menu on shared anchored-menu machinery |
| 11 | dialog | B | 9 | Reachable-P | 1 product: `thread-terminals-card.svelte` (`* as Dialog`, Root/Content/Header/Title/Description :209–249); internal: command-dialog | Root/Trigger/Overlay/Content/Portal/Title/Description/Close + Header/Footer (plain divs) | Overlay `bg-black/80 backdrop-blur-xs fixed inset-0 z-50 duration-100`; Content `bg-popover rounded-4xl p-6 gap-6 ring-1 fixed top-1/2 left-1/2 -translate-x/y-1/2 max-w-[calc(100%-2rem)] sm:max-w-md duration-100`; built-in Close = ghost icon-sm Button + IconX + sr-only "Close" | bits (modal machinery) + wrapper (composition/close affordance) + css | Port modal primitive with overlay, centering, close affordance |
| 12 | dropdown-menu | B | 15 | Reachable-P | 4 product: `project-selector.svelte`, `sidebar-identity.svelte`, `sidebar-engine-usage.svelte`, `thread-environment-card.svelte` | Root/Trigger/Content/Portal/Item/Label/Group/GroupHeading/CheckboxGroup/CheckboxItem/RadioGroup/RadioItem/Separator/Shortcut/Sub* | Content `min-w-48 rounded-2xl p-1 shadow-2xl ring-1 duration-100 z-50 w-(--bits-dropdown-menu-anchor-width) overflow-x-hidden overflow-y-auto`; Item `focus:bg-accent rounded-xl px-3 py-2 gap-2.5`, destructive variant; Label/Shortcut plain div/span (no bits) | bits (menu engine) + wrapper + callsite overrides | Port as primary menu; project picker strips chrome via `t-dropdown` (§6.1) |
| 13 | fade-arc | F | 0 | Reachable-P | 2 product: `thread-route-gate.svelte`, `sidebar-engine-usage.svelte` | FadeArc | Inline SVG spinner, two per-instance linear gradients (`$props.id()` uid), `role="status"`, `animate-[fade-arc-spin_var(--duration,1s)_linear_infinite]`; keyframe animations.css:171–175; attribution comment "Ported from loading-ui's FadeArc (loading-ui.com)" | wrapper + css keyframes | Data-driven drawing: reimplement natively (GPUI arcs + rotation), keep attribution |
| 14 | input | F | 0 | Reachable-P | 2 product: `settings/threads.svelte`, `conversation-prompt.svelte`; internal: input-group-input | Input (file/non-file branches) | `h-9 rounded-4xl border border-input bg-surface-100 dark:bg-surface-900 px-3 py-1 text-base md:text-sm placeholder:text-muted-foreground focus-visible:ring-[3px] ring-ring/50 aria-invalid:ring-destructive/20` | wrapper | Port text-field visual + invalid/disabled states |
| 15 | input-group | F | 0 | **W** → reachable via command | 0 external; sole consumer `command/command-input.svelte` (`* as InputGroup`) | Root/Addon/Button/Input/Text/Textarea | Root `role="group"` `h-9 rounded-4xl border border-input bg-surface-100 focus-within(:focus-visible of control):border-ring ring-ring/50 ring-[3px]`; Addon align variants (tv);Addon click focuses sibling input; Button wraps Button (tv size xs/sm/icon-*); Input/Textarea strip chrome (`rounded-none border-0 bg-transparent shadow-none ring-0`) | wrapper + callsite | Port as field-composition pattern used by command input |
| 16 | lip-card | F | 0 | Reachable-P | 1 product: `thread-composer.svelte` (:544–548) | LipCard (children + `lip` snippet) | `t-acc` accordion utility (utilities.css:476–512): grid-template-rows 0fr→1fr, inner opacity/blur, `data-animate="false"` instant; solid variant `bg-linear-to-b from-surface-200 to-surface-125 dark:from-surface-850 dark:to-surface-900 card`; glass variant drops fill; panel `inert={!open}` | wrapper + css | Port collapse/expand container with inert semantics |
| 17 | native-select | F | 0 | **Dormant** | none (Q5 `\bNativeSelect\b`: barrel only) | NativeSelect/Option | appearance-none select over wrapper div, Selector icon overlay; `has-[select:disabled]:opacity-50`; Option `bg-[Canvas] text-[CanvasText]` | wrapper | Do not port |
| 18 | popover | B | 4 | Reachable-P | 3 product: `model-selector/view.svelte`, `settings/compaction-model.svelte`, `settings/font-picker.svelte` | Root/Trigger/Content/Portal | Content `flex flex-col text-sm duration-100 z-50 origin-(--transform-origin)`, variant default = `card bg-popover gap-4 rounded-2xl p-4 w-72`, variant **bare** = nothing (caller supplies material); defaults sideOffset 4, align center | bits (floating) + wrapper (variants) + callsite material | Port anchored-card primitive with bare/default variants |
| 19 | progress | B | 1 | Reachable-P | 1 product: `context-usage-details.svelte` | Progress (Root + indicator div) | Track `bg-primary/20 h-2 rounded-full overflow-hidden`; indicator `bg-primary transition-all` translated by `-{100-pct}%` | wrapper + bits (value/max) | Port determinate bar |
| 20 | scroll-area | B | 2 | Reachable-P | 1 product: `thread-workspace.svelte` (transcript viewport, :1365–1370) | Root/Scrollbar(+Thumb)/Corner; extra props `viewportClasses`, `scrollbarX/YClasses`, `viewportRef` bindable | Viewport `size-full rounded-[inherit] transition-[color,box-shadow] focus-visible:ring-[3px]` (`cn-scroll-area-viewport` is an inert extension marker with no declaration — §5.10); Scrollbar `w-2.5 data-vertical:border-l touch-none`; Thumb `rounded-full bg-border flex-1` | wrapper + bits + css; Artisan hides all scrollbars (global.css:28–30,57–61; vendor.css:17–19) so visible-bar machinery is effectively unused | Port scrolling container exposing viewport element; scrollbar visuals currently invisible |
| 21 | select | B | 10 (incl. 1 type-only) | Reachable-P | 4 files: product `settings/agent-names.svelte`, `settings/compaction-model.svelte`, `model-selector/policy-controls.svelte`; debug `debug/emulator/+page.svelte` (gallery-only call site) | Root/Trigger/Content/Portal/Viewport(via Content)/Item/Label/Group/GroupHeading/Separator/ScrollUpButton/ScrollDownButton | Trigger `data-size=default/sm h-9/h-8 rounded-4xl border bg-surface-100 dark:hover:bg-input/50` + Selector icon; Content `min-w-36 rounded-2xl shadow-2xl ring-1 duration-100` + viewport `h-(--bits-select-anchor-height) min-w-(--bits-select-anchor-width) scroll-my-1` + scroll buttons (chevrons, `bg-popover z-10 py-1`); Item check indicator IconCheck at `end-2`, `data-highlighted:bg-accent` | bits (typeahead/scroll) + wrapper + tv-free classes | Port listbox selection on shared menu/list machinery |
| 22 | separator | B | 1 | Reachable-P | 1 product: `conversation-status.svelte`; internal: select-separator | Separator | `bg-border shrink-0 h-px/w-px by orientation`; comment records deliberate deviation from shadcn (`vertical:h-full` instead of self-stretch) | wrapper + bits (orientation attr) | Trivial; fold into divider primitive |
| 23 | sheet | B | 8 (Dialog re-wrap) | **Dormant** | none (Q5 `\bSheet\b`: barrel only) | Root/Trigger/Overlay/Content/Portal/Title/Description/Close + Header/Footer (no bits) | Content `fixed z-50 bg-popover shadow-lg transition duration-200 ease-in-out` per-side inset/border + slide-in-from-*-10 open / slide-out-to-*-10 closed; overlay as dialog | wrapper (side variants) + bits Dialog | Do not port; side-panel geometry recorded for future need |
| 24 | shimmer-text | F | 0 | Reachable-P | 4 product: `conversation-message.svelte`, `conversation-status.svelte`, `conversation-trace.svelte`, `conversation-work-session.svelte` | ShimmerText | `t-shimmer-text` utility (utilities.css:764–794): background-clip:text travelling band, `--shimmer-delay/duration/spread` inline vars, 20 named color variants (600/400 light/dark pairs); `active=false` keeps element mounted without animation; reduced-motion turns band off (media query inside utility) | wrapper + css | Port as animated-label treatment; note active-prop rationale (subtree replacement replays entrances) |
| 25 | skeleton | F | 0 | Reachable-P | 4 product: `editor-file-panel.svelte`, `editor-route.svelte`, `sidebar-engine-usage.svelte`, `settings/engine.svelte` | Skeleton | `bg-muted rounded-xl animate-pulse` | wrapper + tw-animate-css pulse | Trivial |
| 26 | slider | B | 1 | **Dormant** | none (Q5 `\bSlider\b`: self only) | Slider (Track/Range/Thumb) | track `rounded-4xl bg-muted h-3`, range `bg-primary`, thumb `size-4 border-primary bg-white ring-4 on focus/hover` | wrapper + bits | Do not port |
| 27 | switch | B | 1 | Reachable-P | 8 product: `conversation-usage-interruption-card.svelte`, `settings/{appearance,engine,notifications,privacy,threads,thread-titles,usage-recovery}.svelte` | Switch (Root+Thumb) | Root `data-checked:bg-primary data-unchecked:bg-input` sizes default 32×18.4px / sm 24×14px, thumb translate-x on checked, `after:-inset-x-3 after:-inset-y-2` extended hit area, focus ring 3px, `transition-all` | wrapper + bits (checked state) | Port toggle with hit-area extension |
| 28 | tabs | B | 4 | Reachable-P | 3 product: `model-selector/view.svelte` (engine tabs), `model-selector/engine-section.svelte` (TabsList/Trigger), `settings/compaction-model.svelte` | Root/List/Trigger/Content; exports `tabsListVariants` (tv) | List tv: default (`bg-muted rounded-4xl p-[3px] h-9 horizontal`) / line (`gap-1 bg-transparent rounded-none`); Trigger `text-sm font-medium rounded-xl px-2 py-1 text-foreground/60 hover:text-foreground data-active:bg-background`, orientation-aware `after:` underline (line variant), `transition-all` | wrapper (tv) + bits (value/orientation) + callsite | Port segmented control; underline variant used by model selector |
| 29 | textarea | F | 0 | **W** → reachable via input-group→command | 0 external; sole consumer `input-group/input-group-textarea.svelte` | Textarea | `resize-none rounded-xl border bg-surface-100 min-h-16 field-sizing-content px-3 py-3 focus ring 3px` | wrapper | Reachable only through Command's textarea member; port if command gains multiline |
| 30 | toggle | B | 1 | **W** → reachable via toggle-group | 0 external; consumers `toggle-group/{toggle-group,toggle-group-item}.svelte` (for `toggleVariants`) | Toggle | tv `toggleVariants`: variants default/outline, sizes default/sm/lg; `aria-pressed:bg-muted hover:bg-muted rounded-4xl` | wrapper (tv) + bits pressed state | Variant recipe needed by toggle-group only |
| 31 | toggle-group | B | 2 | Reachable-P | 1 product: `settings/appearance.svelte` | Root/Item (+ module-level context {variant,size,spacing,orientation}) | Root `data-spacing`/`--gap` driven; Item spacing-0 joins segments (`first:rounded-l-3xl last:rounded-r-3xl`, outline variant drops shared borders); `data-[state=on]:bg-muted` | wrapper + bits (type value) | Port segmented multi/single-select |
| 32 | tooltip | B | 5 | Reachable-P | 8 product files: `composer/controls.svelte`, `model-selector/{view,engine-section,option-tooltip}.svelte`, `onboarding/view.svelte`, `settings/notifications.svelte`, `sidebar-engine-usage.svelte`, `settings/+layout.svelte` (Provider mount) | Provider/Root/Trigger/Content/Portal/Arrow(via child snippet) | Content `inline-flex items-center gap-1.5 rounded-2xl px-3 py-1.5 text-xs bg-foreground text-background z-50 max-w-xs origin-(--bits-tooltip-content-transform-origin)` + kbd-slot helpers; **arrow default true**, rotated square painted bg-foreground, per-side translation classes; Provider default `delayDuration={0}` | bits (delays/grace) + wrapper (arrow rationale comment: layered/glass surfaces opt out) + css `t-tt` family | Port hover-card with delay + grace semantics |

Zero-external-consumer set (dormant + wrapper-internal): `accordion`, `alert`,
`alert-dialog`, `native-select`, `sheet`, `slider` (dormant, confirmed by Q5 negative-space
searches returning only self-hits); `input-group`, `textarea`, `toggle` (wrapper-internal,
inherit reachability: `textarea` ← `input-group` ← `command/command-input`;
`toggle` ← `toggle-group` ← `settings/appearance.svelte`).

Gallery-only call sites (production-stubbed roots reaching production-reachable wrappers):
`debug/components/+page.svelte` and `debug/emulator/+page.svelte` and
`debug/overlay/+page.svelte` → Button; `debug/emulator/+page.svelte` → Select.

Packaging-only reference: `bits-ui` string in `modules/frontend/vite.config.ts:254`
(`optimizeDeps.include`) — build-tool prebundle declaration, never a runtime import site.

## 3. Direct-import ledger (bypasses of the local wrappers)

Every `bits-ui` import outside `lib/components/ui` (Q1, filtered), plus the non-family
symbol and the type-only wrapper-internal edges:

| Site | Symbol | Nature | Notes |
| --- | --- | --- | --- |
| `routes/components/context-usage-gauge.svelte:3` | `LinkPreview` | value import, product, production | No local LinkPreview wrapper exists. Hover-card contract: `Root openDelay={0} closeDelay={120}`, Trigger renders a `<button>` with `aria-label="Context window N% full"` + `aria-describedby="context-usage-details"`, an always-present `sr-only` description span (:58), Portal + Content `side="top" align="start" sideOffset={8}` with `t-tt-presence` motion and `origin-(--bits-link-preview-content-transform-origin)`, ShaderGlassSurface material (:40–78). Full contract §6.5.5. |
| `routes/components/image-viewer.svelte:2` | `Dialog as DialogPrimitive` | value import, product, production | Local Dialog wrapper deliberately bypassed: fills viewport, so primitive dismissal is replaced by an owned full-size dismiss `<button>` layer (:78–84, `tabindex="-1"`, `aria-label="Close image preview"`); overlay `z-50 bg-surface-1000/70 backdrop-blur-md`, content `z-[51]`; sr-only Title; Electron titlebar height offset (:24–26); inspection-store Retain/Release lifecycle (:22–48). Full contract §6.5.2. |
| `lib/components/ui/select/select-separator.svelte:2` | `Separator` (type-only) | wrapper-internal type edge | Pins `SeparatorPrimitive.RootProps` shape; runtime behavior entirely from local Separator wrapper. |
| `lib/components/ui/command/command-dialog.svelte:2` | `Command`, `Dialog` (type-only) | wrapper-internal type edge | Value behavior flows through local Command + Dialog barrels. |
| `lib/components/ui/command/command-group.svelte:2,21` | `useId` | helper import (only non-family symbol) | Generates group values `----${useId()}` when neither `value` nor `heading` given. Bits implementation `<bits>/dist/internal/use-id.js`: module-global counter `globalThis.bitsIdCounter`, returns `` `${prefix}-${n}` `` (prefix `"bits"`). |
| `modules/frontend/vite.config.ts:254` | `"bits-ui"` | packaging-only | optimizeDeps prebundle entry. |

## 4. Wrapper-to-wrapper composition edges (all cross-directory imports, Q3)

All go through barrels or member paths; no relative cross-directory imports exist between
wrapper directories.

```text
dialog/dialog-content.svelte        -> button            (close affordance: ghost icon-sm)
dialog/dialog-footer.svelte         -> button            (optional outline Close)
sheet/sheet-content.svelte          -> button            (close affordance; dormant family)
alert-dialog/alert-dialog-action.svelte -> button        (buttonVariants)
alert-dialog/alert-dialog-cancel.svelte -> button        (buttonVariants, variant outline)
input-group/input-group-button.svelte   -> button
input-group/input-group-input.svelte    -> input
input-group/input-group-textarea.svelte -> textarea
command/command-input.svelte        -> input-group       (* as InputGroup)
command/command-dialog.svelte       -> dialog            (* as Dialog)
select/select-separator.svelte      -> separator
toggle-group/toggle-group.svelte    -> toggle            (toggleVariants)
toggle-group/toggle-group-item.svelte -> toggle          (toggleVariants + types)
popover/popover-content.svelte      -> popover           (member path: popover-portal.svelte)
```

Intra-wrapper relative edges also exist (e.g. each `*-portal.svelte`, `scroll-area.svelte`
importing its own `./index.js` for Scrollbar). Member-level reachability therefore differs
from directory-level reachability exactly where portals are only mounted by siblings.

## 5. Style, token, and motion layer

Entry: `src/lib/styles/global.css` imports tailwindcss, `tw-animate-css`,
`shadcn-svelte/tailwind.css`, then `./theme.css`, `./utilities.css`, `./animations.css`,
`./prose.css`, `./vendor.css`; `@plugin "@tailwindcss/typography"` (:21);
`@custom-variant dark (&:is(.dark *))` (:23). Base layer (:25–66): every element gets
`border-border outline-ring/50`; native scrollbars hidden (`scrollbar-width: none`,
`::-webkit-scrollbar display:none`); `::selection` = `var(--selection)` on
`var(--foreground)`; html/body `min-width 20rem`; heading/code font roles (:48–54).

### 5.1 Color system (`theme.css`)

Semantic tokens resolve onto a 41-step neutral oklch surface ramp (`--surface-0` …
`--surface-1000`, hue ≈285.8, chroma ≤0.017; `theme.css:49–89`). Light theme (:8–148):
`--background: var(--surface-0)` (white), `--foreground: color-mix(in oklch,
var(--foreground-base=--surface-950) 90%, var(--background))`, `--popover: --surface-0`,
`--primary: --surface-900`, `--muted/-foreground: --surface-100/500`, `--accent: --surface-100`,
`--destructive: oklch(0.577 0.245 27.325)`, banner info/error/warning/success
(oklch 0.623 0.214 259.8 / 0.577 0.245 27.3 / 0.681 0.162 75.8 / 0.527 0.154 150.1),
`--favorite oklch(0.706 0.153 78.5)`, `--unread oklch(0.685 0.145 230.318)`,
question purple pair, `--border/--input: --surface-200`, `--ring: --surface-400`.
Dark theme (`.dark`, :265–300): background = `--surface-950`, popover/card = `--surface-900`,
primary = `--surface-200`, border = `oklch(1 0 0 / 10%)`, input = `oklch(1 0 0 / 15%)`.
Sidebar token set (:253–263, :302–311). All exposed to Tailwind via `@theme inline`
(:313–400). Theme switching is `mode-watcher` mounted once in `routes/+layout.svelte`
(import :10, `<ModeWatcher defaultMode="dark" />` :503) toggling the `.dark` class.

### 5.2 Radius ramp, spacing, layout widths (`theme.css`)

- Radius: base `--radius: 0.625rem` (:90); ramp `--radius-xs…4xl` = base × 0.4/0.6/0.8/1/1.4/1.8/2.2/2.6 → **4/6/8/10/14/18/22/26 px** (:386–399, comment declares every corner must sit on this ramp).
- Nested-radius arithmetic: `--radius-nested: calc(var(--radius-surface) - var(--radius-gap, 0px))` (utilities.css:1295 area); the composer sets `[--radius-surface:var(--radius-2xl)] [--radius-gap:calc(var(--spacing)*2)]` (thread-composer.svelte:545,550).
- Reading column: `--prose-width: 48rem` (tight 42rem `[data-prose-width="tight"]`, loose 56rem), `--prose-body-width: calc(--prose-width - 6rem)`, `--prose-gutter: 2rem`, `--prose-rail-gap: 1rem`, `--inspector-width: clamp(16rem, 25vw, 350px)`, `--prose-rail-inset: 2rem`, `--prose-rail-margin: calc(inspector + gap + inset)` (:156–196).
- Card spacing: `--card-spacing: --spacing(6)` (sm: 4) on Card root.

### 5.3 Motion tokens and the reduced-motion authority (`theme.css:91–147`, `animations.css`)

| Token group | Values |
| --- | --- |
| Durations | `--duration-quick:150ms`, `--duration-fast:250ms`, `--duration-medium:350ms` |
| Easings | `--ease-smooth-out: cubic-bezier(0.22, 1, 0.36, 1)`; `--ease-in-out` |
| Dropdown (transitions.dev) | open 250ms / close 150ms, pre-scale 0.97, closing-scale 0.99, `--dropdown-ease` = smooth-out |
| Tooltip (transitions.dev) | `--tt-in-dur:150ms`, `--tt-out-dur:50ms`, `--tt-scale:0.98`, `--tt-delay:80ms`, ease-out both |
| Accordion/lip | `--acc-expand/collapse/chevron: 250ms`, `--acc-ease` smooth-out |
| Panel reveal | open 400ms / close 350ms, translate-y 100px, translate-x −100%−0.5rem, blur 2px |
| Card resize | `--resize-dur:300ms` smooth-out |
| Text swap | 150ms, translateY 4px, blur 2px |
| Stream word | 320ms, rise 0.3em, blur 3px |
| Check burst | 500ms family with rotate-from 80deg, bob 40px, path-delay 80ms |
| Favorite rustle | 350ms, tilt 10deg, pop 1.18, bounce cubic-bezier(0.34, 1.96, 0.64, 1) |

**Reduced-motion authority** (`animations.css:130–160`, single app-wide rule): under
`(prefers-reduced-motion: reduce)` every duration token above is forced to `1ms`
(panel translations/blur to 0), plus blanket `*, *::before, *::after {
scroll-behavior:auto!important; animation-duration:1ms!important;
animation-iteration-count:1!important; transition-duration:1ms!important }`. Component code
additionally checks the media query in JS where behavior (not timing) changes:
`thread-workspace.svelte:512–520` reads `matchMedia("(prefers-reduced-motion: reduce)")` to
disable glide corrections, and `JumpToLatest` picks `behavior:"auto"` vs `"smooth"` (:841–843);
`image-viewer.svelte:96` uses `motion-reduce:transition-none`; `t-shimmer-text` kills its band
inside the utility.

### 5.4 Shadows and materials (`theme.css:206–251`, `utilities.css`)

`--shadow-inset` / `--shadow-inset-artwork` layered inset stacks (:207–226);
hover-surface fill gradient `linear-gradient(to bottom, foreground/16%, foreground/7%)`
(:244–248); `--selection: oklch(0.48 0.13 250 / 42%)` (:250). Utilities:
`@utility card` = `0 -0.5px rgba(255,255,255,.08), 0 4px 8px rgba(0,0,0,.06), 0 0 0 .5px highlight/8%, 0 1px 6px -4px #000` (utilities.css:31–37); `card-lg`, `card-glass`, `card-plastic`,
color-tinted `card-color/info/error/success/warning` variants; `inset-shadow-artwork`.

### 5.5 Typography (`fonts.css`, `theme.css:313–315`)

`--font-sans: "Artisan Neo", sans-serif`; `--font-mono: "JetBrains Mono", monospace`;
`--font-heading: "Artisan Neo"`; `--font-logo: "Cal Sans"`. Variable woff2 `@font-face`
declarations: JetBrains Mono w100–800, Cal Sans w100–1000, Artisan Neo w100–900,
Sigurd Variable w300–900 (wordmark), all `font-display: swap`. Controls are `text-sm`
(14px); composer editor and inputs `text-base md:text-sm`; labels/group-headings/shortcuts
`text-xs`; dialog titles `text-base font-medium leading-none`.

### 5.6 Layering (z-index) inventory

All floating surfaces sit at `z-50` (dialog overlay/content, dropdown/popover/select/
tooltip content, sheet, context menu — each wrapper's content class). The image viewer
raises its content one step: overlay `z-50`, content `z-[51]` (image-viewer.svelte:68,71).
The composer dock floats above transcript at `z-20` (thread-composer.svelte:526); tooltip
arrows `z-50`; select scroll buttons `z-10`. Bits additionally publishes stacking metadata
as CSS variables `--bits-dialog-depth` / `--bits-dialog-nested-count`
(`<bits>/dist/bits/dialog/dialog.svelte.js:273–274`) and applies `contain: layout style`
to dialog and menu content (same file :278; menu.svelte.js:959).

### 5.7 Bits UI attribute contract consumed by Artisan CSS (`vendor.css`)

- `[data-slot="scroll-area-scrollbar"] { display:none }` (:17–19) — hides Bits' scrollbar because Artisan hides scrollbars globally.
- `.t-dropdown[data-popover-content]` motion (:32–76): rest = `scale(0.97)` opacity 0 with open-transition declared; `[data-state="open"]` = `transform:none; opacity:1`; `[data-state="closed"]` = `scale(0.99)` opacity 0 with close transition; `[data-state="starting"]` frame tie-break rule via `[data-starting-style]` (:73–76) so the opening frame starts scaled. Transform origin is taken from `transform-origin: var(--bits-floating-transform-origin)` (:43), the variable Bits publishes but never applies itself (`<bits>/dist/bits/utilities/floating-layer/use-floating-layer.svelte.js:138`). The comment block documents why open must be transform-free (backdrop-filter sampling) — the same reason `t-tt` shown-state is scaleless (utilities.css:431–451).
- `.composer-image-marker` (:83–107) — imperative contenteditable attachment chips (1rem, radius-xs, surface-800, focus outline ring).
- Sonner toaster width variable (:12–14).

### 5.8 Motion utilities used by the selected workflow (`utilities.css`)

`t-tt` (:431–451) hover-card enter/exit keyed on `data-shown`; `t-tt-presence`
(:453–468) the same entrance driven by Bits presence attributes `data-starting-style` /
`data-ending-style`; `t-acc` (:476–512) grid-rows 0fr↔1fr accordion with inner opacity/blur
and `data-animate="false"` bypass; `t-icon-swap` (:317) send/stop icon swap keyed
`data-state="a|b"` and `data-ready`; `t-resize` card resize transitions; `transcript-fade`
(:735–747) top/bottom 16px mask; `docs-scroll-fade` (:539–…) scroll-timeline-driven edge fade
(`@property --docs-scroll-fade-start/end`, animations.css:15–44); `t-shimmer-text`
(:764–794); `placeholder-reveal-in` keyframe (animations.css:223–232) driven by per-character
`--placeholder-delay` (thread-composer.svelte:562–567); `lip-row-grow` (animations.css:292–296)
for queued-steer rows; `status-swap-enter` (:272–278).

### 5.9 Shared wrapper plumbing

`src/lib/utils.ts`: `cn = twMerge(clsx(...))` (:4) plus prop-type helpers `WithElementRef`,
`WithoutChildren(OrChild)`, `WithoutChild`. Every wrapper merges caller classes through `cn`,
so caller utilities (including `!`-important overrides like the project picker's
`bg-transparent! p-0! shadow-none! ring-0! animate-none!`) always win over wrapper recipes.

### 5.10 Pinned dependency style sources (shadcn-svelte 1.4.1 and tw-animate-css 1.4.0, read exactly)

**`shadcn-svelte@1.4.1/dist/tailwind.css`** contains exactly three things: (1) `accordion-down`
/ `accordion-up` keyframes resolving content height from
`var(--bits-accordion-content-height, var(--accordion-panel-height, auto))`; (2) custom
Tailwind variants `data-open`, `data-closed`, `data-checked`, `data-unchecked`,
`data-disabled`, `data-active`, `data-horizontal`, `data-vertical` — each matching
`:where([data-state="…"])` **or** `:where([data-…]:not([data-…="false"]))`; a `data-selected`
variant exists only as a commented-out block; (3) `@utility no-scrollbar`. It defines no
`cn-*` classes.

**The `cn-*` class names are inert extension markers with no declaration anywhere.** Names such
as `cn-scroll-area-viewport`, `cn-tabs-list-variant-default/line`,
`cn-input-group-button-size-sm`, `cn-alert-dialog-action/cancel`, `cn-command-item-indicator`,
`cn-select-item-indicator-icon`, `cn-accordion-trigger-icon`, `cn-card-action` appear only in
wrapper `cn(...)` strings; no `@utility cn-*` or `.cn-*` rule exists in the pinned package CSS,
in Artisan's own stylesheets, or anywhere in the frontend build inputs. Cross-checked against
the sibling checkout's compiled SvelteKit stylesheet (`_layout.BaJXhTOH.css`): every sampled
`cn-*` name is absent from the compiled output while its sibling utilities are present.
Conclusion: all rendered styling of those wrappers comes from the remaining utilities in each
`cn(...)` string; the seams exist so future registry updates or app overrides can attach styles.

**Bare `data-<state>:` utility prefixes compile natively under Tailwind 4.3.2** as attribute
selectors, verified in the same compiled artifact: e.g.
`.data-selected\:bg-muted[data-selected]{background-color:var(--muted)}`,
`.data-highlighted\:bg-accent[data-highlighted]{background-color:var(--accent)}`,
`.data-inset\:pl-9\.5[data-inset]{padding-left:calc(var(--spacing)*9.5)}`,
`.data-placeholder\:text-muted-foreground[data-placeholder]{color:var(--muted-foreground)}`.
So the menu/select/command highlight, inset, and placeholder states cited in §2 and §6 are real
rendered behavior regardless of the package's variant list.

**`tw-animate-css@1.4.0/dist/tw-animate.css`** (single minified line): registers
`--tw-enter-*` / `--tw-exit-*` properties and ships two keyframes — `enter { from { opacity:
var(--tw-enter-opacity,1); transform: translate3d(var(--tw-enter-translate-x,…),…)
scale3d(var(--tw-enter-scale,1),…) rotate(var(--tw-enter-rotate,0)); filter:
blur(var(--tw-enter-blur,0)); } }` and the mirror `exit { to { … } }`. The `animate-in` /
`animate-out` utilities play these with default duration
`var(--tw-animation-duration, var(--tw-duration, .15s))` and default ease `ease`. The modifier
utilities used by wrappers map onto the properties directly: `fade-in-*` →
`--tw-enter-opacity: calc(n/100)`; `fade-out-*` → exit twin; `zoom-in-95` →
`--tw-enter-scale: 95%`; `zoom-out-95` → `--tw-exit-scale: 95%`; `slide-in-from-top-2` →
`--tw-enter-translate-y: calc(2 * var(--spacing) * -1)` (i.e. −0.5rem), with per-side twins;
`duration-100` on contents sets `--tw-duration`. It also ships `accordion-down/up` and
`collapsible-down/up` height keyframes whose fallback chain includes
`var(--bits-accordion-content-height)` / `var(--bits-collapsible-content-height)`.
**The package contains no `prefers-reduced-motion` rule** — reduced-motion behavior comes
solely from Artisan's blanket authority (§5.3), which zeroes these animations through the
global 1ms override.

## 6. Selected initial workflow — behavior contracts

Workflow selected for contract depth: **project picker → thread list & thread creation →
composer → user/assistant messages & transcript**, plus every overlay family those surfaces
actually reach (modal dialog, image-viewer bypass, model-selector popover/tabs/tooltip/
collapsible, LinkPreview bypass, context menus, tooltips, select). Each contract traces
product call site → local wrapper → style source → exact Bits 2.18.1 implementation, and is
attributed per §7.

### 6.1 Shared floating/menu machinery (reached by every anchored surface)

**Bits positioning engine** (`<bits>/dist/bits/utilities/floating-layer/use-floating-layer.svelte.js`
over `@floating-ui/dom`, driven by `use-floating.svelte.js` calling `computePosition`):
middleware chain in order — `offset(mainAxis: sideOffset + arrowHeight, alignmentAxis:
alignOffset)` (:88–91) → `shift({mainAxis:true, crossAxis:false, limiter: sticky==="partial" ?
limitShift() : undefined})` when `avoidCollisions` (:92–98) → `flip()` when `avoidCollisions`
(:99) → `size({apply: capture availableWidth/Height + anchor rect})` (:100–109) → `arrow()`
when an arrow ref exists (:110–114) → `transformOrigin({arrowWidth, arrowHeight})` (:115)
→ `hide({strategy:"referenceHidden"})` when `hideWhenDetached` (:116–117). Repositioning via
`autoUpdate` (:197). Published CSS variables: `--bits-floating-transform-origin`,
`--bits-floating-available-width/height`, `--bits-floating-anchor-width/height`, mirrored
per-family as e.g. `--bits-tooltip-content-transform-origin`
(`<bits>/dist/internal/floating-svelte/floating-utils.svelte.js:16–21`). Artisan consumes the
origin variable in `vendor.css:43` and wrappers consume it as
`origin-(--bits-tooltip-content-transform-origin)` / `origin-(--transform-origin)` /
`origin-(--bits-link-preview-content-transform-origin)`; dropdown content consumes anchor
width as `w-(--bits-dropdown-menu-anchor-width)`, select viewport consumes
`h-(--bits-select-anchor-height)` / `min-w-(--bits-select-anchor-width)`.

**Presence** (`<bits>/dist/internal/presence-manager.svelte.js`): mounts content immediately
on open; on close keeps it mounted with transitionStatus `"ending"` until an
`AnimationsComplete` observation fires, then unmounts and calls `onOpenChangeComplete`;
open transitions flip `"starting"` → cleared one rAF later. Status is rendered as
`data-starting-style` / `data-ending-style` attributes (`getDataTransitionAttrs`), which
`tw-animate-css` classes and Artisan's `t-tt-presence` / `.t-dropdown[data-starting-style]`
rules key on (`animations.css` comment :66–76 documents the first-frame tie-break).

**Dismissal** — EscapeLayer (`<bits>/dist/bits/utilities/escape-layer/use-escape-layer.svelte.js`):
document keydown listener active while enabled; only the top-most layer of the
`globalThis.bitsEscapeLayers` stack handles a given Escape (`isResponsibleEscapeLayer`);
handler dispatches a cloned event to `onEscapeKeydown` and closes unless default-prevented.
DismissibleLayer (`…/dismissible-layer/use-dismissable-layer.svelte.js`): `pointerdown`
captured at document level marks the responsible layer (:84), dismissal fires on the
subsequent `click` outside all enabled layers (:90, :125); menu contents intercept presses on
their own trigger so toggle-clicks don't double-handle
(`<bits>/dist/bits/menu/menu.svelte.js:920–941`, which also sets `ignoreCloseAutoFocus`
so closing via another trigger doesn't steal focus back).

**Focus trap/restore** — Dialog-family content wraps
`FocusScope(loop=true, trapFocus=true)`
(`<bits>/dist/bits/dialog/components/dialog-content.svelte`; scope stack and focus memory in
`<bits>/dist/bits/utilities/focus-scope/focus-scope-manager.js`: register captures
`document.activeElement` as pre-focus memory, unregister resumes the previous scope and
restore targets pre-focus memory). Focusability candidates come from the pinned `tabbable`
dependency (identity from recon: `tabbable@6.5.0` sibling virtual store). Popover content
defaults `trapFocus=true` but disables it for hover-opened popovers until pointer interaction
(`<bits>/dist/bits/popover/components/popover-content.svelte:36–46`,
`effectiveTrapFocus`/`shouldTrapFocus`), and its `preventScroll` default is `false` vs the
dialog's `true`.

**Body scroll lock** (`<bits>/dist/internal/body-scroll-lock.svelte.js`): refcounted lock map;
while any lock is held — body `overflow:hidden`, `pointer-events:none` applied after tick,
padding-right/margin-right compensation by measured scrollbar width unless
`scrollbar-gutter: stable`, `--scrollbar-width` published, iOS touchmove prevention; cleanup
is deferred ~24ms and cancelled if a new lock registers in the same tick (:1639 fix comments).
Dialog content instantiates `ScrollLock {preventScroll:true, restoreScrollDelay:null}` only
while open.

### 6.2 Project picker

Chain: `routes/components/project-selector.svelte` → `ui/dropdown-menu` barrel → Bits
DropdownMenu (menu engine shared with context-menu).

- **State/control**: fully controlled open — local `$state(false)`, `bind:open` on
  `<DropdownMenu>` (:71, :135); item selection and "New project" set `open = false` before
  running their Effect programs (`Choose` :103–108, `NewProject` :110–113); selecting the
  current project closes without side effect.
- **Trigger**: `<DropdownMenuTrigger disabled>` passthrough (wrapper forwards
  `DropdownMenuPrimitive.TriggerProps`); caller sets `aria-label` ("Project: <name>" or custom
  `trigger_label`) and two class shapes (default row vs caller-supplied snippet face)
  (:136–153); hover-pill integration via `pointerenter/pointermove/focusin`. Bits trigger
  semantics: left-click opens (button>0 ignored), Space/Enter preventDefault+open,
  `aria-haspopup="menu"`, `aria-expanded`, `aria-controls=contentId`
  (`menu.svelte.js` DropdownMenuTriggerState :1470; the same value on sub-triggers at :1208).
- **Content geometry**: caller overrides `side="top" align="start" sideOffset={10}`
  (:160–166) vs wrapper defaults (sideOffset 4, align start); width clamp
  `w-[min(20rem,calc(100vw-2rem))] rounded-2xl`; chrome stripped with `bg-transparent!
  p-0! shadow-none! ring-0! animate-none!` because the card's material is a
  ShaderGlassSurface child (:167); motion supplied by the `.t-dropdown[data-popover-content]`
  rules (§5.7) instead of tw-animate-css.
- **Initial focus override**: Bits would highlight the first item; the call site captures the
  row of the *current* project via an attach (`CaptureSelectedProject`, :120–126) and
  `onOpenAutoFocus={FocusSelectedProject}` calls `event.preventDefault()` then
  `selected_project_item.focus({preventScroll:true})` (:128–132). Attribution: behavior =
  Artisan callsite overriding the Bits open-focus hook.
- **Items**: plain items (not radio/checkbox); each carries `onSelect` Effect handlers, hover
  pill handlers, `{@attach FollowHighlight(move_hover)}` shared pill geometry, and suppresses
  default highlight paint (`focus:bg-transparent! data-highlighted:bg-transparent!
  data-highlighted:text-foreground!`) (:185–241); check mark on the chosen project
  (Tabler `check`, aria-hidden); hairline separator span before "New project" (aria-hidden,
  :223–224). Bits item keyboard model (§6.5.7) applies: arrows move focus (loop per
  RovingFocusGroup config), Home/End jump, printable keys typeahead (1s buffer), Enter/Space
  select.
- **Disabled**: `disabled` prop lands on the Bits trigger (`disabled:pointer-events-none`
  comes only if caller adds it; the picker relies on Bits' onclick guard
  `if (opts.disabled.current) return`).
- **Portal/layering**: wrapper always renders `DropdownMenuPortal` (dropdown-menu-content.svelte);
  content `z-50`.
- **Reduced motion**: token zeroing (§5.3) collapses the 250/150ms scale/fade to 1ms.

### 6.3 Thread list and thread creation

Three reachable entry points compose the same navigation semantics:

1. **⌘K/Ctrl+K command palette** — `routes/components/command-menu.svelte` → `ui/command`
   (CommandDialog) → local `ui/dialog` → Bits Dialog + cmdk engine.
   - Toggle: window-level `keydown` handler checks `event.key === "k" && (metaKey||ctrlKey)`,
     `event.preventDefault()` inside `RunBrowserDom`, then flips bound `open`
     (:58–69). Attribution: keyboard binding = Artisan callsite.
   - CommandDialog composition (command-dialog.svelte): `Dialog.Root bind:open` +
     visually hidden header (`Dialog.Header class="sr-only"` with Title/Description — the
     accessible name "Command menu"/description "Search threads and actions") +
     `Dialog.Content` restyled `rounded-4xl! p-0 top-1/3 translate-y-0 overflow-hidden`,
     `showCloseButton=false`; then the cmdk Root (`bg-popover text-popover-foreground
     rounded-4xl p-1 flex flex-col overflow-hidden`). Modal machinery = §6.5.1 (trap, Escape,
     outside press, scroll lock).
   - Input renders through InputGroup (icon addon IconSearch, `h-9 bg-surface-100
     dark:bg-surface-900`), value bindable both ways (command-input.svelte).
   - List: `max-h-72 overflow-y-auto no-scrollbar scroll-py-1`; Empty state text
     "No results found."; groups keyed per project with heading labels; thread rows are
     `<a href={ThreadRoutePathFor(thread)}>` inside CommandItem with searchable value
     `` `${display_title} ${thread.title} ${thread.thread_id}` `` (:97–101) — both title modes
     match by design (comment :96).
   - Ranking/filtering (Bits cmdk): default filter `computeCommandScore`
     (`<bits>/dist/bits/command/compute-command-score.js`, read in full). **Scoring behavior,
     exact**: both sides are lowercased and every whitespace or hyphen character is normalized
     to a plain space before matching (`formatInput`, :120–123); if a call site supplies keywords,
     they are appended to the item value as extra match text (`command + " " + keywords.join("
     ")`, :155–158 — the thread palette passes none, so items match on their `value` only).
     A memoized recursive subsequence matcher then scores the query against the candidate
     with these weights (:8–48): continuous match = 1 per step (`SCORE_CONTINUE_MATCH`);
     resuming at a word start scores 0.9 after spaces, 0.8 after gap punctuation
     (`\ / _ + . # " @ [ ({ &`), and mid-word jumps score 0.17. After matching has begun,
     a mid-word jump is penalized ×0.999 per skipped character; word-start jumps apply that
     penalty once per additional intervening separator. Every original-case mismatch is
     penalized ×0.9999 even though matching itself is case-insensitive. When the query is
     exhausted before the candidate, the path takes one flat ×0.99 incompleteness penalty;
     exhausting both together returns 1. These completion penalties do not compound;
     transposed letters are tried as an alternate path weighted ×0.1. Result range is 0–1,
     1 being a perfect contiguous match from the string start. **Zero semantics**: `#filterItems`
     keeps an item only when `rank > 0` (a full in-order subsequence of the value exists);
     rank-0 items drop out of the count, and any group whose items all rank 0 disappears;
     with an empty search (or `shouldFilter=false`) filtering is bypassed entirely
     (`command.svelte.js:239–270`). `#sort` then orders kept items by descending rank and
     re-inserts groups by their best item's rank (:126–200).
   - Keyboard (exact, from `<bits>/dist/bits/command/command.svelte.js`): root carries
     `role="application"`, `tabindex="-1"`, and the keydown handler (:945–952, :847–944).
     ArrowDown/ArrowUp move the selection one valid item (`#next`/`#prev` :594–605); with
     Meta they jump to last/first (`#last()` :583–585), with Alt to next/previous *group*
     (`updateSelectedByGroup`). Home selects the first valid item and End the last, both with
     `preventDefault` (:918–927). Enter preventDefaults (unless IME-composing, guarded on
     `isComposing || keyCode === 229`) and activates by calling `.click()` on the selected
     item (:928–942) — which is exactly why the palette's `<a href>` rows navigate and why the
     callsite's `is_unmodified_primary_activation` guard sees a real click. Movement wraps
     only if the `loop` option is enabled (`updateSelectedByItem` :446–463); it is optional
     (`types.d.ts`: "Optionally set to `true`"), neither wrapper nor palette call site sets
     it, so **selection stops at the list edges**. Valid items exclude
     `aria-disabled="true"` elements (`getValidItems`, :275+; `itemIsDisabled` :954).
     Vim-style Ctrl+j/k/n/p/h/l bindings exist behind an optional `vimBindings` flag that is
     off here.
   - New thread action: `<a href="/">` guarded by `is_unmodified_primary_activation`
     (unmodified primary click only); `preventDefault()`, then `PrepareNewThreadDraft(...)`
     with `DraftThreadLocked` swallowed (retained first message keeps recovery state but
     navigation still proceeds), close menu, `Navigate("/")` (:41–56, comment :77–80:
     no durable creation from the menu — it jumps to the root draft).
2. **Sidebar rail / new-thread button** — `routes/components/sectioned-panel.svelte`: rail
   visibility `rail_open = thread_rail !== "hidden" && (!threads_loaded ||
   threads.length > 0)` (:116) — an unloaded list holds the rail open (comment distinguishes
   "unknown" from "empty"); StartNewThread (:118–132) mirrors the palette action with
   `new_thread_path = workspace_id ?? "/"` (:99–102).
3. **New-thread route** — `routes/components/new-thread-route.svelte`: resolves project from
   route/workspace or `PreferredProject(recents, …)` (:106–120), aligns the draft controller
   to that project, and composes `ThreadComposer` + `DropdownHoverSurface`
   (:49–51); send gating requires a held project (:207 area).

### 6.4 Composer

Chain: `routes/components/thread-composer.svelte` (+ `composer/controls.svelte`,
`composer/{action-failure,attachment-tray,steering-lip}.svelte`) → `ui/button`, `ui/lip-card`,
`ui/tooltip`, direct `ImageViewer`.

- **Editor**: native `contenteditable` div — `contenteditable={disabled || submitting ?
  "false" : "plaintext-only"}`, `role="textbox"`, `tabindex="0"`,
  `aria-label="Message thread"`, `aria-multiline="true"`, `aria-disabled={disabled ||
  submitting}` (:571–587). Enter submits through a synchronous gesture intake
  (`gestures.SubmitKey`; paste/drop file intake settles DOM synchronously, effects run after
  — comment :477–497). Text persisted to a per-thread draft session across remounts
  (:196–219).
- **Placeholder**: character-by-character reveal — `placeholder-reveal-in` keyframes with
  per-character `--placeholder-delay` computed by `ComposerPlaceholderCharacterDelay`
  (:559–570; keyframes animations.css:223–232), keyed by generation to restart cleanly.
- **Send readiness**: `send_ready` derived from `!disabled && !submitting &&
  send_blocked_reason===undefined && (draft.trim() || attachments) && onsubmit` (:172–178);
  Enter while disarmed reports an ActionFailureAlert instead of silently dropping
  (:363–374). Submit is single-flight via a submit gate (`Acquire`/`Release` :375, :330);
  attachment encoding-in-flight deliberately does not disarm the button (:159–166).
- **Primary control** (composer/controls.svelte): ghost icon-sm Button sized to match the
  model picker (`rounded-(--radius-nested)`), `data-ready={run_active || send_ready}`,
  `disabled` logic switching between stop (abort_available/cancelling) and send; icon swap
  ArrowUp ↔ PlayerStopFilled via `t-icon-swap` `data-state="a|b"`
  (:156–175); wrapped in `TooltipProvider delayDuration={0}` (:145) with tooltip content shown
  only when `send_blocked_reason !== undefined && !run_active` (:179–181).
- **Start-a-new-thread escape hatch**: grid-track reveal `grid-cols-[0fr]→[1fr]` with
  `transition-[grid-template-columns] duration-(--duration-fast) ease-(--ease-smooth-out)`,
  label fades/de-blurs, `tabindex={new_thread_open ? 0 : -1}` (:122–144).
- **Jump-to-latest**: circular ghost Button in a ShaderGlassSurface,
  `aria-label/title "Jump to latest"` (:527–542), mounted only when
  `show_jump_to_latest={!following && !anchor_scroll_active}` (workspace prop, :1527).
- **Steering lip**: LipCard `open={pending.length > 0}`, variant glass↔solid swap
  (:544–548); `t-acc` panel animation (utilities.css:476–512), queued rows enter with
  `lip-row-grow`; panel `inert={!open}` so collapsed controls are unfocusable
  (lip-card.svelte comment + markup).
- **Failure surface**: `ActionFailureAlert` renders `role="alert"` copy
  (composer/action-failure.svelte:32) — dismissible by the callsite's DismissActionFailure.
- **Motion/reduced motion**: all timing rides §5.3 tokens; reduced motion collapses them via
  the authority rule.

### 6.5 Messages and transcript

Chain: `routes/components/thread-workspace.svelte` (+ `conversation-*.svelte`) →
`ui/scroll-area`, `ui/button`, `ui/shimmer-text`, `ui/card/badge/input` (question prompt),
`ui/context-menu`, `ui/progress` (usage details), `ui/skeleton`.

- **Viewport**: `ScrollArea bind:viewportRef class="transcript-fade h-full min-h-0"
  scrollbarYClasses="hidden" viewportClasses="overscroll-contain"` (:1365–1370). The fade
  mask belongs to the frame, not the scroller (comment :1357–1364: masking the scroller broke
  wheel hit-testing). Scrollbar visuals are globally suppressed (§5.7), making Bits' visible
  scrollbar machinery dormant-in-practice; overscroll contained at the viewport.
- **Follow-tail policy** (Artisan-owned; the deepest product behavior in the workflow):
  - `following` starts true (or false when a seeded first submission anchors) and is
    re-derived from scroll position on every unowned scroll (`SyncFollowing`, :478–485) —
    scrolling away disengages, returning to bottom re-engages.
  - New-content pinning is instant (`scrollTo behavior:"auto"`) inside ResizeObserver;
    smooth scrolling is explicitly rejected (mid-flight retargets read as shivering and emit
    intermediate positions that look like reader scrolls) (:963–991 comment + code).
  - Visual glide: corrections ≤56px (`follow_glide_ceiling`, :510) offset content by the
    delta then transition `transform` back to 0 with
    `var(--duration-fast) var(--ease-smooth-out)` (:535–557); disabled under reduced motion
    (:512–520, :536, :551).
  - Sent-message anchoring: submit freezes following; when the user message resolves via its
    source reference, `anchored_user_item_id` claims ownership, end-space height is computed
    to pin the turn at the top inset (`UpdateAnchorLayout` :664–757), smooth pass armed with
    scrollend release + 350ms fallback (0ms under reduced motion) (:726–756); when the end
    space bottoms out within 32px of aligned position, ownership hands back to following
    (:697–711). Wheel/touchstart releases anchor ownership to the reader (:1298–1318);
    programmatic moves are fenced by generation + `scrollend`/timeout (:564–580).
  - Initial placement: thread opens at latest content via assignment, never animation
    (:852–878).
- **History paging**: "Show earlier turns" ghost sm Button, disabled while loading
  (:1380–1392); render window pages 24 groups (:257); hydration refills from durable history
  and compensates scrollTop by scrollHeight delta (:362–393).
- **Turn navigator**: markers re-measured from live geometry on scroll
  (`SyncActiveTurn`, :593–607); selecting a marker hydrates as needed, disables following,
  `scrollIntoView({behavior:"smooth", block:"start"})` (:617–662). Escape exits agent
  inspection (:352–355, :1345).
- **Message semantics**: dispatcher conversation-item.svelte routes item types (:57–87).
  User message `<article aria-label="Your message">` carrying
  `data-conversation-item-id={item.id}` (conversation-message.svelte:188–189); attachments
  show pulse placeholders and view buttons `aria-label={`View ${name}`}` (:200–205);
  assistant/reasoning articles labelled "Assistant message"/"Reasoning summary" (:249);
  streaming labels use ShimmerText whose `active` prop exists so toggling animation never
  swaps the subtree and replays entrances (shimmer-text.svelte doc comment).
  Pending steer announces `role="status"` `aria-label="Steering"` (:229–237).
- **Question prompt** (user turn requiring answers): Card size sm + Badge outline label +
  answer Buttons + free-text Input (conversation-prompt.svelte:57–97).
- **Context menus reached**: thread rail rows expose right-click "Settle" —
  `ContextMenu.Root/Trigger/Content(class w-48)/Item onclick` (thread-hover-rail.svelte
  :590–640); changes cards likewise (conversation-changes-card.svelte). Bits opens on
  right-press via ContextMenuTriggerState (menu.svelte.js:1484+); content recipe identical to
  dropdown-menu-content (§2 row 10).

### 6.6 Overlays actually reached by the workflow

1. **Modal dialog machinery** (used by terminals card and command palette):
   composition per §6 opening — FocusScope(loop, trap) > EscapeLayer > DismissibleLayer >
   TextSelectionLayer > ScrollLock. Overlay visual `bg-black/80 backdrop-blur-xs fixed
   inset-0 z-50 duration-100 fade` (dialog-overlay.svelte); content centered
   fixed top-1/2 left-1/2 translate ±50%, `duration-100` zoom/fade via tw-animate-css
   data-open/data-closed classes; close affordance ghost icon-sm Button + IconX + sr-only
   "Close" (dialog-content.svelte). Terminals card usage: `sm:max-w-3xl gap-3`, Header/
   Title/Description with mono description (thread-terminals-card.svelte:209–249).
2. **Image viewer (direct Bits Dialog bypass)** — image-viewer.svelte: overlay
   `z-50 bg-surface-1000/70 backdrop-blur-md`; content `z-[51]` fills viewport (no
   primitive dismissal possible — owned dismiss button layer `absolute inset-0
   cursor-default tabindex="-1" aria-label="Close image preview"` :78–84); sr-only Title
   names the image; Close is a hover/focus-revealed ghost button in ShaderGlassSurface with
   `motion-reduce:transition-none` (:93–109); Electron titlebar offset variable (:24–26,
   :72); open state reconciles ImageInspectionStore Retain/Release incl. finalizer
   (:33–48). Focus trap/restore + Escape still come from the Bits layers (§6 opening);
   outside-press is unreachable by construction (full-viewport content).
3. **Model selector** — model-selector/view.svelte: outer `TooltipProvider delayDuration={0}
   ignoreNonKeyboardFocus` (:476); Tooltip wraps a span that spreads tooltip props while
   containing the real `PopoverTrigger` button (`aria-label="Select model"`, disabled
   passthrough, focus ring inset) (:480–515); TooltipContent only when a disabled_reason
   exists (:516–518). `Popover bind:open` (:478); `PopoverContent variant="bare"
   align="start" side="top" sideOffset={8}` with material `t-dropdown shader-glass-backdrop
   w-[min(30rem,calc(100vw-2rem))] rounded-3xl animate-none!` and ShaderGlassSurface
   strength strong, `use_backdrop_filter={false}` (:526–538) — i.e. bare variant strips the
   wrapper's default card chrome entirely; motion again via `.t-dropdown` rules. Inside:
   `Tabs bind:value={active_engine}` (:539) with TabsList line-variant triggers
   (engine-section.svelte:74–110, per-engine TabTrigger optionally wrapped in Tooltip showing
   disabled_reason); ModelList uses Collapsible statically `open` with Trigger chevron and
   Content (:170–199); list column `docs-scroll-fade h-48 overflow-y-auto` (:547). Trigger
   rationale comments record why alignment is start/leading (label width changes) and why
   the popover opens upward (:521–525).
   - **Tabs behavior** (`<bits>/dist/bits/tabs/tabs.svelte` + `tabs.svelte.js`): defaults are
     `orientation="horizontal"`, `loop=true`, `activationMode="automatic"` (components/tabs.svelte:15–17).
     Each trigger is `role="tab"` with `aria-selected`, `aria-controls`, `data-state`,
     `data-value`, `data-disabled` (TabsTriggerState.props :139–156); roving tabindex comes
     from the shared RovingFocusGroup configured with the root's loop/orientation (:27–31).
     Keydown: Space or Enter preventDefault+activate; all other keys delegate to roving focus
     (arrows follow orientation, Home/End jump, looping enabled by default)
     (TabsTriggerState.onkeydown :129–138). In `automatic` mode — the mode this workflow runs
     in, since no call site overrides it — merely focusing a trigger activates it
     (onfocus → `#activate` :119–123), so arrowing through engine tabs switches engine per
     keystroke; clicks activate regardless of mode (:124–128). List exposes
     `aria-orientation`/`data-orientation` (TabsListState :76–77).
   - **Collapsible behavior** (`<bits>/dist/bits/collapsible/collapsible.svelte.js`): root is a
     bindable boolean with `toggleOpen`, publishing `data-state=open/closed` and
     `data-disabled`; content presence is `forceMount || open`, mount/unmount timed by the
     same PresenceManager as dialogs (§6 opening), and an optional `hiddenUntilFound` renders
     the closed content with the `hidden` attribute for find-in-page (:12–43, :44–150).
     **Trigger is fully user-toggleable**: primary click toggles (`button !== 0` is only
     prevented, never treated as toggle) and Space/Enter preventDefault+toggle; disabled state
     propagates from root or trigger (:153–190). Trigger publishes `type="button"`,
     `aria-controls=contentId`, live `aria-expanded`, `data-state`, `data-disabled`.
     In the model list, `<Collapsible open>` (model-list.svelte:170) sets the **initial**
     value only — every route-group header remains an open/close control, and its chevron
     rotates with `group-aria-expanded/route:rotate-90` (:177–180). Open/close **does
     animate**: CollapsibleContent carries
     `overflow-hidden data-closed:animate-accordion-up data-open:animate-accordion-down`
     (:183–185). Those variants compile against `[data-state=open]` /
     `[data-open]:not([data-open="false"])` (§5.10), and the compiled rule fixes timing
     exactly: `animation: accordion-down var(--tw-animation-duration,var(--tw-duration,.2s))
     var(--tw-ease,ease-out) …` — **200ms ease-out** default (no duration/ease utility is set
     on this element or its ancestors in the call chain), animating `height` between `0` and
     `var(--bits-collapsible-content-height,…)` (tw-animate-css keyframes, §5.10; sibling
     compiled stylesheet `_layout.BaJXhTOH.css`). Initial-mount semantics are deliberately
     special-cased by `CollapsibleContentState`: `#isMountAnimationPrevented` is initialized
     from the root's open value, the first post-mount measurement forces inline
     `transitionDuration="0s"` and `animationName="none"`, and restoration is skipped while
     that flag is true (`collapsible.svelte.js:44–93`). A requestAnimationFrame then clears
     the guard for later state changes. PresenceManager separately seeds `shouldRender` from
     the initial value and omits a starting transition on its first watch
     (`presence-manager.svelte.js:24–40`). Thus the initially open content paints without an
     entrance animation; subsequent user toggles run the 200ms rules, and a close holds the
     node mounted until AnimationsComplete fires before unmount.
4. **Context-usage gauge (direct LinkPreview bypass)** — context-usage-gauge.svelte:
   `LinkPreview.Root openDelay={0} closeDelay={120}` (:40) — hover-card semantics chosen over
   tooltip so the pointer can move into the reading (:35–39 comment); trigger is a real
   `<button>` (spread preview props) with `aria-label={`Context window N% full`}` and
   `aria-describedby="context-usage-details"` (:43–51); the describedby target is an
   **always-mounted sr-only span** so focused triggers announce without opening (:54–58);
   Portal + Content `side="top" align="start" sideOffset={8}` with `t-tt-presence` motion and
   `origin-(--bits-link-preview-content-transform-origin)`, chrome stripped for the
   ShaderGlassSurface material (:67–77). **Hover behavior, exact** (from
   `<bits>/dist/bits/link-preview/link-preview.svelte.js`): open fires on trigger
   `pointerenter` (touch ignored) or on keyboard focus only when `:focus-visible`
   (:122–138), then `handleOpen` starts the open timer — `openDelay` ms, cancellable
   (:78–89); pointer leaving *before* the card is mounted/open cancels immediately
   (`immediateClose`, :127–132). Close after opening is scheduled at `closeDelay` ms
   (`handleClose` :95–103) but suppressed while the pointer is down on the content or a text
   selection exists inside it (:18–21, :184–193). Keeping the pointer between trigger and
   card is handled by Bits' **SafePolygon** — instantiated by LinkPreviewContentState with
   `onPointerExit → handleClose` (:172–179); SafePolygon tracks the pointer against the
   polygon spanned by the trigger and content rectangles and fires exit only when the pointer
   leaves that hull, with a transit-intent timeout and rAF fallback cancelling stale transits
   (`<bits>/dist/internal/safe-polygon.svelte.js`: polygon/rect tests :5–26, listeners
   :150–160, rect-exit checks :176–187). So with this call site's `openDelay={0}
   closeDelay={120}`: the card opens instantly on hover or keyboard focus, survives diagonal
   pointer travel to itself, and closes 120ms after the pointer finally exits both rects.
   Escape closes via the popper layer's `onEscapeKeydown → handleClose`; outside interaction
   likewise (:202–213). The content is non-focusable by design: `tabindex="-1"`, all tabbable
   candidates inside are forced `tabindex="-1"` while open, and open/close auto-focus are both
   prevented (:61–64, :214–219, :226). Trigger semantics published:
   `aria-haspopup="dialog"`, `aria-expanded`, `role="button"` (:142–155).
5. **Tooltips** — provider scopes (all verified): settings layout
   `delayDuration={0} ignoreNonKeyboardFocus` (settings/+layout.svelte:21); composer controls
   `delayDuration={0}` (controls.svelte:145); sidebar engine usage `delayDuration={0}`
   (sidebar-engine-usage.svelte:290); onboarding `delayDuration={150}`
   (onboarding/view.svelte:235); model selector `delayDuration={0} ignoreNonKeyboardFocus`
   (view.svelte:476). Wrapper Provider defaults delayDuration 0 (tooltip-provider.svelte).
   Bits behavior: effective delay = root ?? provider (tooltip.svelte.js:179); provider
   skipDelayDuration lets pointer moves between triggers reopen without the full delay
   (:112–142); open on pointer enter after delay or immediately on keyboard focus
   (#onfocus :545); content stays hoverable unless `disableHoverableContent`, with a
   skipDelayDuration=0 special case (:599–618); Escape closes content (:648); presence attrs
   drive `data-state=delayed-open|instant-open` styling hooks used by the wrapper classes
   (`data-open:`/`data-[state=delayed-open]:` entrances, tooltip-content.svelte). Arrow:
   rotated square painted bg-foreground, per-side translation overrides, opt-out for
   layered/glass materials (tooltip-content.svelte doc comment :14–20, markup :40–58).

### 6.7 Select and remaining reached primitives (settings adjacency)

Select (settings agent-names/compaction-model, policy controls): trigger sizes default/sm
with Selector icon; content portals with `preventScroll=true` default (select-content.svelte),
viewport sized from anchor vars, auto scroll buttons repeat-scroll toward the overflow edge
(`SelectScrollDownButtonState.scrollIntoViewTimer`, select.svelte.js:1182+); keyboard: FIRST_KEYS
ArrowDown/PageUp/Home, LAST_KEYS ArrowUp/PageDown/End (:17–18), trigger typeahead via
DOMTypeahead plus DataTypeahead for string values (:555–600), Enter/Space toggle, Escape
closes via EscapeLayer; item check indicator IconCheck; highlighted item scrolled into view
(`node.scrollIntoView({block: scrollAlignment})` :99). **Positioning**: bits-ui 2.18.1's
Select has no `position` option at all — `SelectContent` always renders through `PopperLayer`
(`<bits>/dist/bits/select/components/select-content.svelte`: import :5, render :65), i.e. the
Radix-style item-aligned mode does not exist in this version and the floating middleware chain
of §6 opening (offset/shift/flip/size) always applies. The Artisan wrapper passing no such prop
is therefore moot; select content is always trigger-anchored, collision-avoided, and sized by
the anchor variables the wrapper consumes (`h-(--bits-select-anchor-height)`
/ `min-w-(--bits-select-anchor-width)`). Switch/ToggleGroup/Tabs/Collapsible/Progress/Separator/
Avatar contracts are as recorded in §2 (state ownership: bindable checked/pressed/value/open
through Bits boxes; visuals: wrapper recipes); Tabs/Collapsible specifics for the selected
workflow are in §6.6 item 3.

## 7. Source-of-behavior attribution and uncertainty register

Attribution summary for the selected workflow:

| Concern | Owner | Evidence anchor |
| --- | --- | --- |
| Open/close state of menus/popovers/dialogs | Bits (bindable box) controlled by Artisan callsite state | project-selector :135; command-menu :71; view.svelte :478 |
| Which item is initially highlighted | Artisan callsite (overrides Bits open-focus) | project-selector :115–132 |
| Roving focus, typeahead, Home/End, submenu intent, dismissal interception | Bits menu engine | menu.svelte.js :849–901, roving-focus-group.js, dom-typeahead.svelte.js |
| Floating position/collision/origin variables | Bits floating layer (@floating-ui/dom middleware chain) | use-floating-layer.svelte.js :87–118 |
| Open/close presence (mount/unmount timing, starting/ending frames) | Bits PresenceManager | presence-manager.svelte.js |
| Entrance/exit *visuals* of anchored surfaces | Artisan CSS (`.t-dropdown` vendor rules, `t-tt*` utilities) replacing tw-animate-css on glass surfaces; tw-animate-css elsewhere | vendor.css :32–76; utilities.css :431–468 |
| Modal modality: trap, restore, scroll lock | Bits (FocusScopeManager, BodyScrollLock) invoked by wrapper composition | dialog-content.svelte; body-scroll-lock.svelte.js |
| Follow-tail, anchoring, glides, paging | Artisan (thread-workspace) | §6.5 citations |
| Send readiness/single-flight/failure reporting | Artisan (thread-composer) | §6.4 citations |
| Command ranking/filtering | Bits cmdk port (computeCommandScore) | command.svelte.js :120–200 |
| ⌘K binding, thread navigation, draft locking | Artisan callsite | command-menu.svelte :41–69 |

Remaining uncertainties (plain list; none blocks the contracts above):

1. **Production-closure proof is static.** Reachability rests on query-based import
   resolution over this worktree's source, cross-checked against per-wrapper reading,
   dormancy negative searches, dynamic-import enumeration, and compiled-CSS spot checks in
   the sibling install — not against a compiler-produced module graph, since no JS toolchain
   is installed here.

Resolved during review of this document (now recorded as evidence): the shadcn-svelte 1.4.1
and tw-animate-css 1.4.0 style sources including the inert `cn-*` markers (§5.10); Select's
popper-only positioning (§6.7); exact command-palette keyboard semantics (§6.3) and the full
`compute-command-score` formula with its zero/nonzero semantics (§6.3); Tabs/Collapsible
state, focus, toggle, and animation behavior (§6.6 item 3); LinkPreview SafePolygon hover
contract (§6.6 item 4). The stray noncompiled typo artifact `routes/debug/overlay/+page.sv`
is excluded from all counts as recorded in §1.

## 8. Companion artifacts

- **`docs/ui/GPUI_CAPABILITIES.md`** (preceding Phase 6 stack artifact): owns the
  usable / insufficient / absent verdicts for pinned GPUI facilities against exactly the
  behaviors recorded here — focus entry/trap/restoration, keyboard traversal and typeahead,
  anchored placement/collision, overlay stacking, scroll containment and follow-tail
  feasibility, presence windows, reduced-motion policy, and accessibility roles. This
  inventory deliberately does not duplicate those verdicts (PLAN lines 594, 628).
- **`docs/ui/ASSETS.md`** (preceding Phase 6 stack artifact): owns the complete SVG-use
  census, including the Tabler glyphs referenced inside wrapper members (IconX, IconCheck,
  IconMinus, IconChevronDown/Up/Right, IconSelector, IconSearch — referenced from census rows
  above as style/composition dependencies only) and the classification of `fade-arc` as a
  data-driven drawing for native reimplementation (loading-ui.com attribution preserved).
- Cross-reference direction: GPUI_CAPABILITIES and ASSETS may cite INVENTORY rows; INVENTORY
  asserts nothing about their contents.

## 9. Native-port guidance (Phase 7 inputs)

- **Preserve**: the visual token system of §5 verbatim (surface ramp, radius ramp, motion
  tokens, shadows, typography roles, selection color, dark-mode values, `defaultMode="dark"`);
  the behavior contracts of §6 including dormancy decisions — dormant families (accordion,
  alert, alert-dialog, native-select, sheet, slider) are recorded evidence, not ports.
- **Design idiomatic GPUI APIs; do not mechanically port**: Svelte component decomposition,
  `$bindable` props (model as entity state and explicit events/actions), SER/Effect programs
  (typed async methods and owned tasks), Bits attribute contracts (`data-state`,
  `data-slot`, `--bits-*` variables are behavioral evidence, not an API surface), CSS masks
  (transcript/shadow fades need explicit GPUI-native equivalents or a recorded deferral), and
  portal/DOM focus machinery (map to GPUI focus handles and overlay layers).
- **Shared-framework boundary** (PLAN §ui): primitives enter `modules/ui` only when the
  audited call sites here require them; product policy (follow-tail, anchoring, draft
  lifecycle, steering lips, ranking presentation) stays in `frontend` until a second consumer
  proves abstraction.
- **Shader-backed materials are deferred**: ShaderGlassSurface appears throughout the
  selected workflow (picker card, model-selector card, gauge card, image-viewer close,
  composer/jump surfaces). Phase 6 implements no shader and no shared GPUI primitives; native
  treatment of these surfaces is decided in Phase 7 within ordinary GPUI styling constraints.
- **Reduced motion** must remain a first-class setting: the legacy authority (token zeroing +
  blanket 1ms) and the JS media-query checks (glide suppression, scroll behavior choice)
  together define the observable contract to reproduce.
