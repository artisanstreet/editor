# Native port pull-request stack

The native Artisan Editor port is a full product rewrite delivered as a long stack of small pull requests. It is not a five-PR project. The expected order of magnitude is roughly sixty to seventy or more PRs, but the final count is deliberately not fixed before the work is understood.

## Stack rules

- Each PR implements one reviewable work packet or one tightly coupled proof.
- A PR targets the branch immediately below it in the stack. It targets `master` only when it is the bottom of a new stack after the prior stack has been merged by the maintainer.
- Sequence numbers in branch names express ordering, not a final total. Use names such as `port/04-protocol-transport-proof` without labels such as `4/70`.
- PRs remain drafts until their own implementation and evidence are ready. Automation never merges them; the maintainer decides when and how to merge.
- The controlling agent creates, edits, inspects, and closes PRs with `gh`. Git remains responsible for local commits, branches, rebases, and pushes.
- Parallel work happens in isolated worktrees with disjoint ownership. Its commits are reviewed and integrated serially into the active stack so shared manifests and interfaces keep one history.
- Root Bazel/Cargo state, lockfiles, shared schemas, and public crate interfaces have one integrating writer at a time.
- Every PR body names its immediate dependency, scope, exclusions, tests, and remaining known work. A later PR must not hide an unfinished requirement in an earlier one.
- When the maintainer merges a lower PR, rebase the next live branch onto the new `master`, retarget its PR with `gh`, and then repair the branches above it in order.

## Likely packet families

The stack will grow across foundation and feasibility proofs, domain and protocol slices, QUIC transport behavior, SeaORM migrations and repositories, Forge capabilities, asset extraction, UI archaeology, shared GPUI primitive families, frontend workflows and screens, accessibility and visual reviews, packaging, and eventual legacy retirement.

Those families describe decomposition, not bulk milestones. A protocol schema, a migration family, a UI primitive family, or one end-to-end capability can each require multiple PRs when that keeps review and rollback clear.

## Current bottom of stack

1. Native port plan.
2. Bazel-native Rust workspace foundation.
3. This stack policy.

Feasibility and product packets continue above this point. The list is intentionally open-ended.
