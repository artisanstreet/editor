# Barekey Design Language Inspection

This note inspects the public [`usebarekey/barekey`](https://github.com/usebarekey/barekey) frontend at commit [`2811067`](https://github.com/usebarekey/barekey/tree/2811067b396af392e45814145aa00a302079e91f). It describes the marketing and documentation frontend that exists in that repository, not an unseen Barekey product dashboard. Route usage is treated as stronger evidence than components that merely exist in the shadcn inventory.

## Read This Before Reusing Anything

The repository has no `LICENSE`, `LICENCE`, `COPYING`, or `NOTICE` file, and GitHub reports no detected license. Public source is available to inspect, but that is not permission to copy, modify, or redistribute its code or bundled assets. Artisan should reproduce the high-level principles with its own tokens, component code, typography, logo, and illustrations. Do not copy Barekey CSS or Svelte components, and do not reuse its logos or bundled PP Neue Montreal files without explicit permission.

## The Short Version

Barekey's visual language is calm, technical, and almost monochrome. It builds hierarchy with luminance, opacity, typography, and fine surface treatment instead of many colors. The outer composition is generous, while controls and navigation are compact. Large rounded panels, hairline rings, subtle vertical gradients, and carefully layered shadows keep the interface soft without making it glossy.

The most useful direction for Artisan is the combination of a restrained dark-first shell, dense operational controls, and motion that preserves spatial continuity. Artisan needs a stronger semantic color layer because it must communicate agent state, permissions, diffs, conflicts, and ownership at a glance.

## Foundations

### Color

Barekey uses an almost neutral zinc palette expressed in OKLCH. Light mode starts with white and a near-black foreground; dark mode starts at `oklch(0.141 0.005 285.823)` with slightly raised `0.21` cards. Most supporting surfaces are small luminance steps in the same hue family. Destructive red, link and code blue, note variants, and the occasional badge provide the limited chromatic accents. Dark mode is the default.

This produces a clear rule: color means something because ordinary hierarchy does not consume it. Artisan should keep that rule, but define its own semantic colors for success, running, waiting, warning, blocked, destructive, added, removed, conflict, and agent attribution.

Sources: [global tokens](https://github.com/usebarekey/barekey/blob/2811067b396af392e45814145aa00a302079e91f/modules/frontend/src/lib/styles/global.css#L12-L95), [default color mode](https://github.com/usebarekey/barekey/blob/2811067b396af392e45814145aa00a302079e91f/modules/frontend/src/routes/+layout.sv#L14-L22).

### Typography

Barekey assigns four explicit jobs to type:

- PP Neue Montreal is the body face.
- Geist is the heading face, normally semibold with `-0.03em` tracking.
- Cal Sans is the logo face with `-0.05em` tracking.
- JetBrains Mono is the code face.

Marketing headings are light and large at roughly 48–60px, while documentation headings are compact and semibold. Prose uses a 74ch maximum measure and balanced or pretty wrapping. Artisan's PRD already selects Cal Sans for its wordmark, so the useful lesson is the role separation rather than Barekey's exact font stack. Artisan should source Cal Sans independently under a verified license and choose licensed body, heading, and mono faces before implementing typography tokens.

Sources: [font roles](https://github.com/usebarekey/barekey/blob/2811067b396af392e45814145aa00a302079e91f/modules/frontend/src/lib/styles/fonts.css#L67-L84), [prose width](https://github.com/usebarekey/barekey/blob/2811067b396af392e45814145aa00a302079e91f/modules/frontend/src/lib/styles/prose/base.css#L23-L33), [landing heading](https://github.com/usebarekey/barekey/blob/2811067b396af392e45814145aa00a302079e91f/modules/frontend/src/routes/components/landing-page.sv#L100-L118).

### Spacing and Density

The shell leaves room around major surfaces, but operational UI stays tight: buttons are 24, 32, 36, or 40px tall; navigation rows are 28px; most icons are 16px; major desktop surfaces are separated by 8px; and panel padding steps through 20, 24, and 28px. This makes the interface feel calm without reducing information density.

Artisan should use the same split personality. Pane gutters and content margins can breathe, while thread rows, tab strips, status rows, terminal controls, and right-pane sections should remain compact enough for daily developer use.

Sources: [button sizes](https://github.com/usebarekey/barekey/blob/2811067b396af392e45814145aa00a302079e91f/modules/frontend/src/lib/components/ui/button/button.sv#L20-L29), [documentation navigation density](https://github.com/usebarekey/barekey/blob/2811067b396af392e45814145aa00a302079e91f/modules/frontend/src/routes/docs/%5Bcategory%5D/%5Bslug%5D/components/docs-sidebar.sv#L439-L524).

### Surfaces

Panels use faint vertical gradients, half-pixel or one-pixel rings, inset top highlights, and several low-opacity shadow layers rather than heavy borders or flat filled cards. The base radius is 10px and scales to roughly 26px; buttons are usually pills. Documentation panels use a four-pixel outer frame around an inset rounded content layer, which makes neighboring surfaces legible even when their colors are close.

For Artisan, use three surface levels: the application canvas, pane surfaces, and elevated transient surfaces. Avoid turning every right-pane section or message into a separate floating card; hierarchy should come from spacing and fine dividers until an element genuinely needs elevation.

Sources: [radius and shadow tokens](https://github.com/usebarekey/barekey/blob/2811067b396af392e45814145aa00a302079e91f/modules/frontend/src/lib/styles/global.css#L39-L65), [card shadows](https://github.com/usebarekey/barekey/blob/2811067b396af392e45814145aa00a302079e91f/modules/frontend/src/lib/styles/global.css#L133-L166), [layered documentation surfaces](https://github.com/usebarekey/barekey/blob/2811067b396af392e45814145aa00a302079e91f/modules/frontend/src/routes/docs/%5Bcategory%5D/%5Bslug%5D/components/docs-page-frame.sv#L71-L102).

## Composition

Marketing pages use a centered `max-w-6xl` canvas with 24–32px horizontal padding and a small number of large content groups. Documentation is the closer reference for Artisan: at 1280px and above it becomes a three-column workspace with a 16rem collapsible left navigation, a fluid article, a fixed 350px right table of contents, 8px gutters, and independent masked scroll regions.

Below 1280px, the documentation layout becomes normal document flow with a sticky glass header, table of contents, then article. Navigation moves into an 18rem edge-attached sheet. This is a sensible pattern for a website, but Artisan is a desktop application whose main work surface must remain useful. Its responsive order should be fluid main pane first, then collapse the right pane, then reduce the left pane to an icon rail or overlay. It should not stack terminal, Git, and session controls above the editor.

Sources: [documentation frame](https://github.com/usebarekey/barekey/blob/2811067b396af392e45814145aa00a302079e91f/modules/frontend/src/routes/docs/%5Bcategory%5D/%5Bslug%5D/components/docs-page-frame.sv#L46-L105), [sidebar constants](https://github.com/usebarekey/barekey/blob/2811067b396af392e45814145aa00a302079e91f/modules/frontend/src/lib/components/ui/sidebar/constants.ts#L12-L29), [mobile sheet](https://github.com/usebarekey/barekey/blob/2811067b396af392e45814145aa00a302079e91f/modules/frontend/src/lib/components/ui/sidebar/sidebar.sv#L37-L65).

## Controls and Icons

The component language starts from shadcn-svelte's `maia` style and Bits UI behavior. Buttons are rounded, borderless by default, use subdued fills, carry a visible three-pixel focus ring, and move down by one pixel on press. Inputs follow the same 36px pill treatment; menus use 16px corners, four-pixel internal padding, and 32–36px rows. Tabler's thin outline icons dominate at 16px, while third-party logos are desaturated so they do not break the neutral hierarchy.

Artisan should keep the consistent 16px icon rhythm and explicit focus treatment. Pill controls fit toggles, filters, identities, and compact actions, but file tabs and dense data rows need flatter geometry so the whole application does not become a collection of capsules.

Sources: [component preset](https://github.com/usebarekey/barekey/blob/2811067b396af392e45814145aa00a302079e91f/modules/frontend/components.json#L1-L19), [button variants](https://github.com/usebarekey/barekey/blob/2811067b396af392e45814145aa00a302079e91f/modules/frontend/src/lib/components/ui/button/button.sv#L9-L35), [menu surface](https://github.com/usebarekey/barekey/blob/2811067b396af392e45814145aa00a302079e91f/modules/frontend/src/lib/components/ui/dropdown-menu/dropdown-menu-content.sv#L19-L30), [icon use](https://github.com/usebarekey/barekey/blob/2811067b396af392e45814145aa00a302079e91f/modules/frontend/src/routes/components/landing-page.sv#L5-L10).

## Motion

Motion is tokenized around 150, 250, and 350ms with a smooth-out cubic curve. Sidebar resize takes 300ms, mobile panel entry and exit use 400 and 350ms, and child rows stagger inside bounded windows. Movement often combines translation, opacity, and two or three pixels of blur. The bespoke sidebar and table-of-contents systems explicitly remove their transitions under reduced motion rather than merely shortening them.

The important principle is continuity: a highlight moves between navigation items, panes visibly change size, and disclosure content expands from its actual location. Artisan should use 150–250ms for routine feedback, reserve longer motion for pane topology changes, never animate high-volume terminal or transcript updates, and provide a deterministic reduced-motion state for every bespoke transition.

Sources: [motion tokens](https://github.com/usebarekey/barekey/blob/2811067b396af392e45814145aa00a302079e91f/modules/frontend/src/lib/styles/global.css#L40-L65), [sidebar motion](https://github.com/usebarekey/barekey/blob/2811067b396af392e45814145aa00a302079e91f/modules/frontend/src/lib/styles/sidebar.css#L1-L55), [bounded stagger](https://github.com/usebarekey/barekey/blob/2811067b396af392e45814145aa00a302079e91f/modules/frontend/src/lib/client/sidebar-motion.ts#L1-L37).

## Distinctive Patterns Worth Translating

### One Moving Navigation Highlight

The documentation sidebar uses one shared gradient hover highlight and a separate one-pixel selected caret. The highlight moves between measured rows instead of giving every row a permanent filled background. Artisan can translate this to threads, Marketplace navigation, and settings sections, provided keyboard focus remains independently visible.

Source: [sidebar highlight and caret](https://github.com/usebarekey/barekey/blob/2811067b396af392e45814145aa00a302079e91f/modules/frontend/src/lib/styles/sidebar.css#L128-L183).

### Technical Framing

`MarketingFrame` extends one-pixel guide lines beyond a flat content region and places crosshair marks at two corners. It gives otherwise quiet content an editorial, toolmaking character. Artisan can adapt this sparingly for onboarding, empty workspaces, or review summaries; repeating it around every pane would add noise.

Source: [marketing frame](https://github.com/usebarekey/barekey/blob/2811067b396af392e45814145aa00a302079e91f/modules/frontend/src/routes/components/marketing-frame.sv#L14-L29).

### Masked Scroll Edges

Independent desktop scroll regions fade at their top and bottom edges. This quietly communicates that more content exists without adding persistent scroll chrome. It fits Artisan's thread list and right-pane stacks, but terminal and editor surfaces should keep their native scrolling cues.

Source: [scroll masks](https://github.com/usebarekey/barekey/blob/2811067b396af392e45814145aa00a302079e91f/modules/frontend/src/routes/docs/%5Bcategory%5D/%5Bslug%5D/components/docs-page-frame.sv#L108-L129).

### Code as a First-Class Surface

Inline code is a small pill. Code blocks are large rounded gradient surfaces with filename chips, copy actions, line numbers, and shaped per-line diff highlights rather than blunt full-width bands. Artisan should carry that care into tool output and markdown, but Monaco and the dedicated diff viewer should own primary editing and review rather than imitate documentation code cards.

Sources: [inline code](https://github.com/usebarekey/barekey/blob/2811067b396af392e45814145aa00a302079e91f/modules/frontend/src/lib/styles/prose/inline-code.css#L4-L16), [code blocks](https://github.com/usebarekey/barekey/blob/2811067b396af392e45814145aa00a302079e91f/modules/frontend/src/lib/styles/markdown/components/code-snippet.css#L3-L54).

### Glass Only Where It Helps Structure

The compact header uses translucent gradients, inset light, backdrop blur, and a floating pill, while most of the site stays opaque. Artisan should follow that restraint: blur is suitable for a temporary overlay, command palette, or pane switcher above moving content, but not as the base material of every panel.

Source: [compact glass header](https://github.com/usebarekey/barekey/blob/2811067b396af392e45814145aa00a302079e91f/modules/frontend/src/routes/docs/%5Bcategory%5D/%5Bslug%5D/components/docs-mobile-header.sv#L6-L18).

## Artisan Translation Rules

- Build an Artisan-owned neutral palette and surface system; do not reproduce Barekey's token values wholesale.
- Keep chroma scarce in ordinary chrome so operational state colors remain immediately legible.
- Preserve the PRD's `272px minmax(720px, 1fr) 340px` desktop layout. Barekey validates the general three-pane grammar, not Artisan's exact pane sizes.
- Use generous pane framing with dense 28–36px controls and 16px icons.
- Use an independently sourced, properly licensed Cal Sans build only for the Artisan wordmark, as already specified. Select licensed faces for every other typography role.
- Use moving highlights and spatial transitions where they explain continuity; keep focus rings, status changes, and reduced-motion states explicit.
- Keep terminal output, diffs, agent state, permission requests, and conflicts functional before decorative. These surfaces carry more state than Barekey's documentation UI.
- Treat Barekey as mood and interaction research. Every production component must be independently designed and implemented for Artisan.
