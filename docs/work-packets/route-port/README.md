# Route-port work packets — GPUI visual parity with the legacy editor

## Problem

The native frontend compiles and runs, but its screens do not look like the
legacy Artisan editor. Prior work optimized for passing state-machine checks
instead of reproducing the product. These packets re-port each route as a
direct translation of the legacy Svelte surface into GPUI.

## Ground rules (every packet)

1. **The legacy app is the specification.** Reference root (read-only, never
   edit): `C:\Users\sander\Desktop\artisan-editor\modules\frontend\src\`.
   Trace the route's `+page.svelte` through its child components in
   `routes/components/` and its `$lib/` controllers. Do not infer from
   component names; read the markup and styles.
2. **Element-tree fidelity.** The GPUI element tree nests in the same order as
   the Svelte DOM tree. Every wrapper div with layout utility classes maps to
   a `div()` with equivalent `Styled` calls (`h-dvh` → `.h_full()`, `flex` →
   `.flex()`, `shrink-0` → `.flex_shrink()`, `w-14` → `.w(px(56.))`,
   `min-h-0` → `.min_h_0()`, `flex-1` → `.flex_1()`). Structural
   paraphrasing (flattening wrappers, reordering, swapping flex direction) is
   rejected even if it renders plausibly.
3. **Tokens, not eyeballed hexes.** Colors, radii, font sizes, and spacing come
   from `modules/ui/src/theme.rs` (`ArtisanTheme`, `ThemeMode`) and, where a
   token is missing, from the legacy `lib/styles/theme.css`/`global.css`
   values added once to the screen module as named constants with the legacy
   source in a comment. Do not edit `modules/ui/**` to add tokens; report the
   gap instead.
4. **Reuse the shared framework.** `artisan-ui` already ships button, card,
   input, textarea, select, popover, dialog, sheet, dropdown_menu, tooltip,
   switch, slider, badge, avatar, separator, skeleton, shimmer_text, tabs,
   progress, icon, markdown, scroll_area, and more. Use them where the legacy
   surface used the corresponding wrapper; do not hand-roll copies.
5. **Ownership is exclusive.** Touch only the files listed in your packet plus
   (if genuinely required) new files you add under
   `modules/frontend/src/`, registered in `modules/frontend/BUILD.bazel`.
   Never edit: `native_application.rs`, `native_route.rs`,
   `native_transport_service.rs`, `Cargo.toml`, root `BUILD.bazel`,
   `modules/ui/**`, `modules/assets/**`, or another packet's files. The
   orchestrator owns those and integrates serially.
6. **Ship a screen entry point.** Expose a public view type in your module,
   e.g. `pub struct OnboardingScreen { .. }` implementing `gpui::Render`, plus
   a constructor taking the state it needs. The orchestrator wires it into the
   root route switch. Do not wire it yourself.
7. **Checks.** `cargo check -p artisan-frontend`, `cargo fmt -p artisan-frontend -- --check`,
   and `cargo clippy -p artisan-frontend -- -D warnings` must pass in your
   worktree. A packet that does not compile is not done.
8. **Visual evidence.** For the default route, run
   `powershell -File scripts/verify_visual.ps1 -Name <route>` in your worktree
   and attach the PNG path. For other routes, evidence is the fidelity mapping
   (see below); the orchestrator captures integrated screenshots after wiring.
9. **Fidelity mapping.** Your report includes a table: each significant Svelte
   element/component → the GPUI element/component that replaces it, with the
   Tailwind classes → `Styled` calls noted. Anything you could not reproduce
   is listed explicitly as a gap. Claimed parity without a mapping is
   rejected.
10. **No reward hacking.** Do not simplify the surface, invent UI the legacy
    route does not have, hide unwired state behind placeholder boxes without
    marking them, or relax checks. A missing detail stated honestly beats a
    plausible fake.

## Packets

| # | Branch / worktree suffix | Legacy sources (old repo) | Native target |
| --- | --- | --- | --- |
| 1 | `route-shell-vp1` | `routes/+layout.svelte`, `routes/components/sidebar-identity.svelte`, `thread-hover-rail.svelte`, `workspace-header.svelte`, `lib/root/shell-layout.ts` | `shell.rs`, `shell_layout.rs`, `shell_presentation_state.rs`, `thread_hover_rail_policy.rs`, `workspace_header_presentation.rs` |
| 2 | `route-new-thread-vp1` | `+page.svelte`, `t/[workspace]/+page.svelte` → `components/new-thread-route.svelte`, `thread-composer.svelte`, `project-selector.svelte`, `sidebar-engine-usage.svelte`, `lib/root/*` | `native_new_thread_surface.rs`, `native_thread_picker.rs`, `project_picker.rs`, `composer.rs`, `native_composer.rs`, `new_thread_*.rs` |
| 3 | `route-thread-vp1` | `t/[workspace]/[thread]/+page.svelte` → `thread-route-gate.svelte`, `thread-route.svelte`, `thread-workspace.svelte`, `thread-panel.svelte`, `conversation-*.svelte`, `lib/conversation/*` | `conversation_host.rs`, `transcript.rs`, `conversation_*.rs`, `thread_title_policy.rs`, `active_thread_light_policy.rs` |
| 4 | `route-editor-vp1` | `e/[workspace]/[thread]/+page.svelte` → `editor-route-gate.svelte`, `routes/components/editor-route.svelte`, `editor-file-panel.svelte`, `workspace-file-tree.svelte`, `thread-workspace.svelte` | `editor_route_screen.rs` (new), `editor_*.rs`, `vcs_*.rs` |
| 5 | `route-onboarding-vp1` | `onboarding/+page.svelte` → `components/onboarding/view.svelte`, `setup-label.svelte`, `setup-state.ts`, `lib/onboarding-route.ts` | `onboarding_screen.rs` (new), `onboarding_route.rs`, `onboarding_harness_presentation.rs`, `harness_setup_policy.rs` |
| 6 | `route-settings-vp1` | `settings/+layout.svelte`, `settings/**/+page.svelte` → `components/settings/*` | `native_settings.rs` (rewrite), `engine_settings.rs`, `*_settings_policy.rs` |

## Integration order

Shell integrates first (it is the frame all screens mount in), then
new-thread, thread, onboarding, settings, editor. After each integration the
orchestrator runs the full check set, captures `verify_visual.ps1` evidence,
and fast-forwards `master`.
