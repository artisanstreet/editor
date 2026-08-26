# Native port pull-request stack

The native Artisan Editor port is a full product rewrite delivered as a long stack of small pull requests. It is not a five-PR project. The expected order of magnitude is roughly sixty to seventy or more PRs, but the final count is deliberately not fixed before the work is understood.

## Stack rules

- Required delivery order: create an isolated worktree, implement and verify, publish the branch and stacked PR, merge the accepted branch into LOCAL `master`, then remove the completed inactive worktree. Sander confirmed this exact order on 2026-08-26. See root `AGENTS.md` for safe retirement checks.
- Local integration is not remote integration. Continue publishing stacked PRs; never merge a GitHub PR, enable auto-merge, or push local `master` to remote `master` without a new explicit request. Keep the branch refs needed by open PRs after deleting their worktrees.
- Each PR implements one reviewable work packet or one tightly coupled proof.
- A PR targets the branch immediately below it in the stack. It targets `master` only when it is the bottom of a new stack after the prior stack has been merged by the maintainer.
- Every published PR must also belong to the managed GitHub stack. Correct base branches alone do not satisfy delivery. If using `gh pr create`, append with `gh stack link <active-stack-number> <new-PR-number>`, import the new PR with `gh stack checkout <new-PR-number>`, and verify the remote head/base plus `gh stack view --json` before local merge or worktree retirement.
- A platform size limit may require a managed continuation whose trunk is the prior stack's last PR branch. This segmentation does not merge that dependency or justify targeting `master`. Preserve the old stack and record the continuation; never omit registration. Avoid `gh stack sync` just to refresh tracking because it can rebase/push and local `master` is the integrated tip, not the remote trunk.
- Sequence numbers in branch names express ordering, not a final total. Use names such as `port/04-protocol-transport-proof` without labels such as `4/70`.
- PRs remain drafts until their own implementation and evidence are ready. Automation never merges them; the maintainer decides when and how to merge.
- The controlling agent creates, edits, inspects, and closes PRs with `gh`. Git remains responsible for local commits, branches, rebases, and pushes.
- Parallel work happens in isolated worktrees with disjoint ownership. Its commits are reviewed and integrated serially on candidate stack branches so shared manifests and interfaces keep one history. Verify and publish each candidate PR before advancing local `master` to it.
- Root Bazel/Cargo state, lockfiles, shared schemas, and public crate interfaces have one integrating writer at a time.
- Every PR body names its immediate dependency, scope, exclusions, tests, and remaining known work. A later PR must not hide an unfinished requirement in an earlier one.
- When the maintainer merges a lower PR, rebase the next live branch onto the new `master`, retarget its PR with `gh`, and then repair the branches above it in order.

## Active managed continuation — 2026-08-26

Stack **#19** retains its 98 entries through PR **#114**,
`port/98-pinned-client-session`. GitHub rejected appending all six of
PRs #115–#120 because the resulting stack exceeded its size limit.
Those PRs are now registered in continuation stack **#121**, whose trunk
is `port/98-pinned-client-session`; their immediate-parent bases are
unchanged. Append new packets to **#121**, not #19, until a newer verified
handoff records another continuation. No remote PR was merged to create it.

## Likely packet families

The stack will grow across foundation and feasibility proofs, domain and protocol slices, QUIC transport behavior, SeaORM migrations and repositories, Forge capabilities, asset extraction, UI archaeology, shared GPUI primitive families, frontend workflows and screens, accessibility and visual reviews, packaging, and eventual legacy retirement.

Those families describe decomposition, not bulk milestones. A protocol schema, a migration family, a UI primitive family, or one end-to-end capability can each require multiple PRs when that keeps review and rollback clear.

## Current bottom of stack

1. Native port plan.
2. Bazel-native Rust workspace foundation.
3. This stack policy.

Feasibility and product packets continue above this point. The list is intentionally open-ended.
