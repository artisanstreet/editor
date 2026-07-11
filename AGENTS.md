# Artisan Editor Agent Instructions

## Start Here

- Read `MEMORY.md` before planning, editing, delegating, reviewing, or committing.
- Treat `MEMORY.md` as the persistent handoff for current progress, unresolved findings, and next work.
- Update `MEMORY.md` after every substantive milestone, review, newly discovered blocker, verification run, and before handing work to another session.
- Keep `backend-completion-matrix.md` aligned with verified implementation status. Do not mark a slice implemented from intent or narrow tests.

## Required Skill

- Always use [`sanders-skill`](C:/Users/Sander/.codex/skills/sanders-skill/SKILL.md) for work in this repository.
- Follow its TypeScript, Effect, structure, tooling, Git, and delegated-development references.
- Use Effect Services and Layers extensively for capabilities, infrastructure, shared state, and lifecycle ownership. Do not pass dependency bundles through ordinary function parameters.
- Before implementing any capability, always research whether Effect already provides an API, module, package, Service, Layer, platform adapter, or experimental implementation for it. Prefer the Effect implementation when it exists, including Effect AI for model and provider integrations, and document why a custom boundary is necessary when Effect does not cover the requirement.

## Subagent-Driven Development

- Use subagent-driven development for non-trivial work. The main Sol session is the coordinator and owns architecture, task decomposition, integration, user communication, and final verification.
- Never use fast mode or a priority service tier for subagents.
- Use Terra at medium reasoning for ordinary implementation, debugging, integration tests, and substantial reviews.
- Use Luna at medium reasoning for mechanical edits, inventories, focused tests, formatting, and tightly bounded grunt work.
- Reserve Sol for coordination, critical architectural or release reviews, and a new Sol subagent only when another worker is genuinely blocked and cannot be unblocked by a clearer brief or Terra.
- Give each worker a bounded objective, exact file ownership, constraints, and verification commands. Workers must not edit overlapping files concurrently.
- Tell every worker that other work may exist in the worktree and that it must not revert or overwrite changes it did not make.
- Keep delegation one level deep. Workers report to the coordinator and do not spawn their own workers unless explicitly authorized.
- Independently review substantial or high-risk changes before committing. The coordinator performs the final requirement audit and full validation.

## Workflow Guardrails

- Work on the existing feature branch unless `MEMORY.md` says a new branch is required.
- Use pnpm and the repository scripts. Run `pnpm run validate` before a milestone commit.
- Use `apply_patch` for manual file edits.
- Do not start a development server unless explicitly requested.
- Do not discard, reset, or revert dirty work that is not yours.
- Commit coherent, verified milestones. Never push to `main` or `master` without explicit approval.
