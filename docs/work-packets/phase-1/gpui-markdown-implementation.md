# Phase 1 implementation packet: upstream GPUI and Markdown engine proof

You are a bounded implementation worker in an isolated Git worktree. Do not spawn subagents.

Read `docs/PLAN.md` completely, especially the first-party error-policy and streaming Markdown sections. Then read `%TEMP%\artisan-editor-opencode-phase1-current\bazel-gpui.report.md` as reconnaissance input rather than authority. Correct it where the current plan supersedes it.

## Objective

Implement the Phase 1 native-host feasibility proof for upstream GPUI and the chosen Markdown engines. Prove real compile-time and runtime API use without designing Artisan's unaudited visual system or building disposable browser compatibility.

The controller will integrate root dependency declarations, `MODULE.bazel`, workspace lockfiles, aggregate targets, and the final stacked PR.

## Exclusive file ownership

You may create or edit only:

- `modules/ui/**`
- `modules/frontend/**`
- `tests/ui/**`
- this packet file

Do not edit root files, lockfiles, `MODULE.bazel`, other modules, `.github`, or `docs/PLAN.md`.

## Non-negotiable decisions

- Use the upstream GPUI crate. Do not fork, patch, vendor, or copy GPUI to remove its transitive dependencies.
- `anyhow` is prohibited only in first-party manifests and source. Third-party dependencies may use it internally.
- Use `pulldown-cmark` for Markdown parsing and `syntect` for fenced-code syntax highlighting. Do not implement a Markdown grammar or programming-language grammar.
- Do not depend on `gpui-component`, Zed's internal Markdown crate, a WebView, DOM, HTML renderer, Svelte, Node, or Electron.
- Bazel is authoritative. Cargo manifests exist for metadata and dependency resolution, not as the product build driver.

## Required proof

- Pin a concrete upstream GPUI release compatible with this host and use it through real APIs, including application/window startup, an actual renderable GPUI element, actions or other relevant macros, and native platform dependencies. An unused dependency is not proof.
- Add the smallest honest native frontend proof window that can be launched manually. Mark feasibility-only presentation clearly; do not invent Artisan layout, tokens, components, or interaction behavior before the Bits UI/UI archaeology phase.
- Give `modules/ui` a narrow Markdown engine seam that genuinely invokes `pulldown-cmark` and `syntect` and produces owned semantic/highlight data suitable for a later GPUI renderer. Keep the model deliberately minimal and avoid claiming the final renderer or streaming coordinator is implemented.
- Parse at least headings, paragraphs, inline code, fenced-code boundaries, and raw HTML as inert data. Highlight at least one closed Rust fence through `syntect`, using byte ranges or equivalent owned ranges rather than emitting HTML.
- Keep transcript pacing, canonical-message ownership, correction handling, and generation cancellation out of `ui`; those remain later `frontend` product work.
- Add external tests under `tests/ui/` for the implemented Markdown seam, including raw HTML inertness, open-fence fallback, closed-fence highlighting, and deterministic owned output. Tests use the normal harness and `#[test]`/`#[gpui::test]` only as actually supported.
- No `anyhow` in first-party manifests or source. Use typed errors and `thiserror` only where an error crosses a useful boundary.
- All hand-written Rust uses edition 2024 and the root lint configuration. Keep production code free of `unwrap`, `expect`, `panic!`, `todo!`, and `unimplemented!`.
- Do not add shaders or claim visual fidelity. That work follows source archaeology.
- Use `apply_patch` for hand-written edits. Run rustfmt and the focused checks possible without controller-owned lockfile integration. Review `git diff --check` and the complete diff, then commit once. Do not push, create a PR, or merge.

## Report

Report the commit SHA, exact upstream crate versions/features selected, files changed, commands and results, controller integration still required, native-host limitations, and which portions are only feasibility evidence rather than completed product UI.

## Implementation record (2026-08-24)

The worker implementation was integrated into the authoritative Bazel graph and
verified on Windows. The final stack commit supersedes the worker-only commit
SHA; its source changes remain attributable to worker commit
`38c271178aee24b9fc556126f174ab3b619be669`.

- `gpui = 0.2.2`, with the upstream `inspector` feature enabled. Bazel builds
  procedural macros in its optimized execution configuration while the normal
  library has debug assertions enabled. GPUI's official feature makes its
  debug-assertion-gated reflection macro available in both configurations;
  this avoids a fork, patch, or first-party copy.
- `pulldown-cmark = 0.13.4`, with default features disabled.
- `syntect = 5.3.0`, with default features disabled and `parsing`,
  `default-syntaxes`, and `regex-fancy` enabled.
- The proof window uses real upstream GPUI startup, window creation, rendering,
  focus, pointer handling, actions, and keyboard bindings. A manual smoke run
  opened the titled proof window, remained alive for four seconds, then closed
  normally with exit code zero.
- The Markdown seam produces owned heading, paragraph, inline-code, inert-HTML,
  fence, and semantic code-token data. An open fence remains plain; a closed,
  recognized fence uses `syntect`; an unknown language remains unhighlighted.
  Seven external tests cover those boundaries and deterministic output.
- Worker-side scratch verification passed six tests, strict Clippy, formatting,
  GPUI closure compilation/linking, and the native-window smoke run. Controller
  integration then passed `cargo metadata --locked --no-deps`,
  `bazel build //...` for 22 targets, `bazel test //...` for seven tests,
  `bazel build //:clippy`, and `git diff --check`.
- The GPUI renderer for Markdown, streaming coordinator, generation/correction
  state, theme-color mapping, code-block UI, math, and Mermaid are intentionally
  not implemented by this feasibility slice. Those remain later first-party
  product work governed by `docs/PLAN.md`.
- This proves the normal Windows Bazel fastbuild/debug path. Release shader
  handling is still deferred with the rest of Artisan's shader work; this
  packet makes no release-mode or visual-fidelity claim.
- The observed future-incompatibility notice originates in the third-party
  `proc-macro-error2` closure. It does not add `anyhow` or a policy exception to
  first-party manifests or source.
