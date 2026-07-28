# Archived Branch Inventory

Last updated: 2026-07-28

This document records branches that were removed from the repository after
verification, and what each one still holds. Every branch listed here is
preserved as an annotated-free `archive/*` tag, so the commits remain reachable
and are never garbage collected. Nothing recorded here has been deleted.

To inspect an archived branch:

```sh
git log archive/<name>
git diff master archive/<name> -- <path>
git switch --detach archive/<name>
```

## Fully superseded — archived, no action needed

The following branches were verified file-by-file against `master`. Every path
they touch exists on `master`, and where the content differs `master` holds the
newer revision. They were milestone branches whose work landed before the
`acea0f3` rehabilitation restructure.

| Tag                                     | Unique commits | Milestone                                 |
| --------------------------------------- | -------------- | ----------------------------------------- |
| `archive/m1-protocol-ledger-rebuild`    | 4              | Protocol ledger and projection rebuild    |
| `archive/m2-artisan-tool-control-plane` | 10             | Built-in tool control plane               |
| `archive/m5-orchestration-surfaces`     | 5              | Orchestration and activity surfaces       |
| `archive/m7-marketplace`                | 4              | Marketplace registries and MCP transports |
| `archive/m9-deep-harness-release`       | 13             | Deep harness and release validation gates |

The only files these branches carry that `master` lacks are the 49 files that
the rehabilitation milestone deliberately deleted — root `MEMORY.md`, the root
product and research documents that moved under `docs/`, and the dormant
`@artisan/bounded-file-store-native` package.

Branches deleted with no archive tag, because they contained zero commits that
were not already reachable from `master`: `codex/backend-services`,
`preview-backend-completion`, `candidate`.

## `archive/workspace-replace-approval` — unmerged feature work

This is the only archived branch holding work that never reached `master`.

- Diverged from `master` on 2026-07-13 at `3b7fb03`; last commit 2026-07-17.
- 150 commits, 246 files that have never existed anywhere in `master`'s history.
- A direct merge is not viable: it conflicts in 208 files and predates the
  entire `acea0f3` rehabilitation restructure, including the typed
  `ControlRpcGroup` wire contract that replaced the plumbing it was built on.

Treat this branch as a **feature backlog, not a merge candidate**. Each area
below should be reimplemented against the current architecture, taking the
design and test cases as reference rather than cherry-picking commits.

### Absent from `master` and potentially wanted

| Area                       | Paths on the archived branch                                                                                                                                                                    |
| -------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Git hosting providers      | `modules/backend/src/git-provider/` — provider registry, transport authentication, GitHub provider, `gh` CLI integration                                                                        |
| Hosted git mutations       | `hosted-git-mutation-coordinator`, `hosted-git-mutation-repository`, `hosted-git-snapshot-{repository,service}`                                                                                 |
| Local git sessions         | `workspace-git-{session,fetch,checkout}-*`, `workspace-git-observer`, `workspace-git-execution-gate`, `git-fetch`, `git-mutation`, `node-git-mutation`                                          |
| External wait              | `modules/backend/src/external-wait/` — coordinator, dispatcher, policy, repository, service                                                                                                     |
| Export control             | `modules/backend/src/compliance/` — export control, intent commitment, SQLite audit store                                                                                                       |
| Hosted project clone       | `modules/backend/src/projects/hosted-project-clone-*`, `project-repository`, `project`                                                                                                          |
| Preview browser            | `modules/backend/src/preview/preview-browser*`, `preview-target-repository`                                                                                                                     |
| Harness context            | `modules/backend/src/harness/harness-context.ts`                                                                                                                                                |
| Workspace replace approval | `workspace-replace-approval-{coordinator,repository}`                                                                                                                                           |
| Editor shell UI            | `modules/frontend/src/routes/components/` — `editor-shell`, `editor-workspace`, `file-tab-strip`, `quick-open`, `left-pane`, `main-pane`, `right-pane`, `mode-switcher`, `workspace-navigation` |
| Visual fixture route       | `modules/frontend/src/routes/visual-fixtures/`                                                                                                                                                  |

The branch also carries 13 protocol codecs with no counterpart on `master`:
`capability`, `export-control`, `external-wait`, `git-mutation`, `git-session`,
`hosted-git`, `hosted-project`, `local-git-fetch`, `marketplace-routine-control`,
`rich-link`, `surface`, `terminal-tools`, `tool-control`.

### Deliberately excluded — do not revive

- `modules/engines/src/claude/` (`claude-engine`, `claude-jsonl`,
  `claude-normalizer`) — revived on `master` on 2026-07-28. The codex-only
  boundary and its enforcement test
  (`.tests/backend/codex-only-production.test.ts`) were deliberately retired as
  part of that revival; the adapter now lives at `modules/engines/src/claude/`
  under the current Engine contract and is registered in Forge alongside Codex.
- `modules/backend/src/filesystem/native-bounded-regular-file-store.ts` and the
  `@artisan/bounded-file-store-native` package, removed by the rehabilitation
  milestone as dormant.

### Reimplemented independently on `master`

These existed on the archived branch and were later rebuilt under different
module names. Compare before assuming anything is missing.

| Archived branch                     | Current `master` equivalent               |
| ----------------------------------- | ----------------------------------------- |
| `modules/backend/src/tool-control/` | `modules/backend/src/tools/`              |
| `modules/backend/src/surface/`      | `modules/backend/src/surfaces/`           |
| Rich link transport and metadata    | `modules/backend/src/preview/rich-link-*` |
| Terminal tooling                    | `modules/backend/src/terminal/`           |
