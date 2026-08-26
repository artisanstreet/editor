# Artisan editor agent workflow

Read `docs/SESSION_HANDOFF.md` for the current checkpoint and `docs/STACK.md`
for the native-port PR chain before continuing work. The workflow below
records Sander's explicit clarification on 2026-08-26 and supersedes older
handoff statements that prohibit pushing branches or opening PRs.
The handoff is intentionally local/untracked in the canonical checkout; if
it is absent from a worker checkout, obtain the current checkpoint from VP.

## Required delivery order

1. Create an isolated, bounded Git worktree for an implementation packet.
2. Implement and verify the packet there. Preserve unrelated work, review the
   actual changes, and record the checks run on the final source bytes.
3. Publish the verified branch and open a PR targeting the immediately
   preceding branch in the existing stack. Verify the remote head and base.
4. When the packet is acceptable, merge it into this project's **local
   `master`**. Prefer a fast-forward of the verified stack branch. This local
   integration is not a remote merge and does not wait for a remote merge.
5. Delete the completed, inactive worktree after confirming its work is
   preserved in the PR and local integration. Keep the branch refs needed
   by the open PR stack. Do not retain one checkout/build cache per old PR.

Never merge a GitHub PR, push local `master` to remote `master`, or enable
auto-merge without a new explicit request. **Publishing stacked PRs is
required; "merge locally" never means "stop publishing PRs."**

Shared-interface/manifest integration and its combined checks may be done
serially on a candidate stack branch. Do not advance local `master` with
that candidate before its PR is published. Preserve small, coherent commits;
do not batch completed packets into a giant PR.

## Orchestration and machine limits

- The root agent is VP: orchestrate PMs, review/integrate results, verify and
  publish the stack. PMs orchestrate external implementation workers using
  the CLI/model currently authorized by the user and recorded in the handoff.
- Prefer Fable High through `claude` for system architecture and concrete
  worker-ready plans: interfaces, dependencies, file ownership, invariants,
  and acceptance tests. This is Sander's explicit 2026-08-26 preference. Let
  already-running implementation workers finish; do not silently change
  worker models or use Codex as an implementation fallback.
- Reuse the existing PM lanes. Target a load-balanced roughly 5–10 PMs, not
  an always-full quota. No recursive Codex-agent fan-out or silent model
  fallback. Never create hundreds of agents or worktrees.
- Let live workers finish naturally; do not kill them to reclaim slots.
- Current conservative native-build policy: one native gate at a time,
  `--jobs=1`, fresh three-sample actual CPU mean below 85%, and at least
  6 GiB free RAM before starting. Reduce concurrency under pressure. Do not
  use stale CPU readings as current load, or modify unrelated processes.

## Safe worktree retirement

- Resolve the exact registered worktree path and verify its HEAD/status.
  Never recursively remove a workspace root or an unresolved path.
- Verify no worker, test, compiler, or session still depends on that checkout.
- Confirm all work is committed and preserved. For cherry-picked worker
  commits, verify the integrated source and retain an explicit recovery ref
  to the original commit before retirement.
- Use ordinary `git worktree remove` without force for a clean inactive tree.
  Dirty or unfinished work must be preserved and investigated, not discarded.
- Retire only that inactive tree's verified build outputs, using the build
  tool's own cleanup where possible. Preserve active/shared caches, logs,
  recovery archives, branch refs and all unrelated user files.
- Record the PR, local integration commit, retired path, and recovery ref in
  the handoff. Carry this policy and current lane/queue state across compaction.
