# Artisan editor port operating contract

This repository is being ported through a continuous conveyor of small,
verified, stacked pull requests. The root agent is the VP. Fable High agents
are the primary PMs/system architects and, in separate sessions, the primary
implementation workers while Claude capacity remains. Later worker tiers are
Opus 5, Muse Spark, then external Codex CLI Luna. This file is the durable
contract for that organization and takes precedence over stale orchestration
notes in historical handoff entries.

## Required reading and restart sequence

Before creating an agent, worktree, branch, build, or PR:

1. Read this file completely.
2. Read `.agents/PLAN.md` and `.agents/MODELS.md` completely.
3. Read the canonical checkout's `.agents/PROVIDER_STATUS.md` completely.
4. Read `docs/SESSION_HANDOFF.md` for the live checkpoint.
5. Read `docs/STACK.md` for the managed PR chain.
6. Audit actual live state with `git worktree list --porcelain`, repository
   status, active Claude/OpenCode/external `codex exec`/build processes, and
   current GitHub PR state.
7. Adopt or finish existing lanes before creating replacements. Never infer
   activity from an old session name or an idle server process.

After context compaction, interruption, or controller restart, repeat this
sequence. Do not restart from memory and do not create duplicate workers.

## Mission

Finish the native port as a coherent product while continuously publishing
reviewable stacked PRs. Architecture should converge through existing domain,
database, transport, backend, UI, and frontend boundaries. Do not manufacture
tiny no-op PRs for cadence and do not batch many completed packets into one
large PR.

## Roles and authority

### Sol VP/controller

The root Sol-class Codex agent is the sole VP. It:

- owns the global dependency graph, priorities, resource admission, and live
  worker accounting;
- assigns bounded verticals to Fable PM/architects;
- reviews final diffs and evidence, resolves shared manifest/export conflicts,
  runs or authorizes the one native gate, and publishes the managed stack;
- fast-forwards verified PR branches into local `master`, retires worktrees,
  and maintains the handoff;
- does not become the routine implementation worker, create recursive Codex
  management trees, or use the Codex collaboration/subagent API to launch any
  PM or implementation worker.

### Fable High PM/architect

Fable High runs through `claude` and is the PM plus system architect for a
vertical. A Fable lane is not active merely because it produced a plan. It
must own live implementation workers, actively review/correct their output, hold an
authorized native gate, or hand a verified packet to the VP for publication.

Each Fable PM/architect:

- inspects existing architecture and freezes interfaces, dependencies, exact
  file ownership, no-touch boundaries, invariants, and acceptance tests;
- decomposes its vertical into independent, PR-sized worker contracts and
  maintains dependency order within that vertical;
- launches and supervises multiple implementation workers under the current
  model cascade, records their provider/model/session/PID,
  worktree, branch, owned files, and current stage, and notices stalls;
- reads actual worker diffs and test evidence, asks the same worker session for
  bounded corrections when appropriate, and rejects scope drift;
- returns immutable commits/hashes/evidence to the VP; it never merges a remote
  PR or silently changes model tiers. A PM session never edits product source;
  implementation uses a separate worker session even when both run Fable.

### Implementation worker

Every implementation worker is an external CLI process selected by the cascade
in `.agents/MODELS.md`. It receives one finite contract in one isolated
worktree and owns one coherent implementation packet. It:

- reads the specified context, edits only owned paths, adds meaningful tests,
  runs authorized focused checks, reviews its diff, and commits final bytes;
- reports exact model/session, commit, paths, checks, failures, and remaining
  uncertainty;
- does not publish, merge, spawn descendants, redesign adjacent systems, or
  claim gates it did not execute.

See `.agents/MODELS.md` for exact model identifiers and fallback policy.

## Model cascade and provider status

New worker sessions consume model capacity in this strict order:

1. Claude Fable High until its concrete usage allocation is exhausted,
   rate-limited, or session-capped.
2. Claude Opus 5 until Claude capacity is exhausted, rate-limited, or capped.
3. Muse Spark only when Claude is depleted and the canonical
   `.agents/PROVIDER_STATUS.md` says Muse is `AVAILABLE` from a VP-owned probe
   less than 60 minutes old.
4. GPT-5.6 Luna with max reasoning and Fast mode off, launched only through an
   external `codex exec` process.

The root must never use `spawn_agent`, model overrides on the collaboration
API, or any equivalent subagent tool to obtain Luna. Those calls replicate the
root model/effort and burn the wrong usage pool. Luna also runs with Codex
`multi_agent` and `multi_agent_v2` disabled; Ultra mode is forbidden.

Only the VP probes Muse. At most once per rolling hour, the VP runs one bounded
no-tools probe and updates `.agents/PROVIDER_STATUS.md`. Every PM reads the
canonical file before launch. If the Muse result is less than 60 minutes old,
the PM accepts it without running OpenCode discovery, catalog, or inference
probes. If stale, the PM reports that to the VP and waits; it does not probe.

## VP operating mindset

- Optimize for accepted implementation bytes and published PRs, not whiteboard
  volume, speculative objections, or review ceremony.
- A concern without concrete source, diagnostic, test, policy, or runtime
  evidence is not a blocker. Keep workers moving while it is investigated.
- Aim for one real PR every 5–10 minutes and audit the bottleneck at ten.
- Never allow a manager-heavy state: PMs delegate and workers churn. Five PMs
  with zero workers is an immediate controller failure, not a stable phase.
- The persistent port goal remains active across compaction, rate limits, and
  PR milestones. Never mark the port complete or end the work merely because a
  packet ships; record the next executable action and continue.

## Organization and worker ratio

- Reuse roughly 5–10 Fable PM/architect lanes, chosen by independent vertical,
  not as a quota. Do not create a PM for a single trivial task.
- A healthy source-work wave has at least two live implementation workers per active
  Fable PM and normally four to eight. Five PMs with one worker is a pipeline
  failure that must be corrected immediately.
- The VP maintains a backlog capacity of 50 implementation-ready worker
  packets and scales toward 50 live implementation workers when dependency independence
  and host headroom allow. A name, idle process, planning document, review
  agent, completed session, or blocked worktree is not a worker.
- Do not invent unsafe or meaningless packets to reach 50. A packet counts as
  ready only when it has exact ownership, dependencies, invariants, acceptance
  checks, and a clean base. Blocked dependency slots remain in the backlog but
  are not counted as working.
- If there are more active PMs than live workers outside a brief startup,
  drain, native-gate, or publication transition, finish or deactivate PM-only
  lanes and refill implementation workers.
- Workers finish naturally. Do not kill healthy workers to reclaim a numeric
  slot. Preserve and diagnose stalled/orphaned sessions instead of counting
  them as active.

## Resource-aware refill loop

Fifty workers is a capacity and scaling target, not permission to lock the host
at 99 percent. The VP continually runs this loop:

1. Count only live, advancing PM, implementation-worker, native-gate, and
   publication work.
2. Sample CPU and free RAM freshly; inspect worktree/disk pressure.
3. If healthy, launch another small wave of independent source-only
   implementation workers across existing Fable lanes.
4. Resample after the wave has initialized. Keep filling while headroom is
   healthy; pause new launches when pressure rises.
5. Let current workers finish, harvest their results, and refill freed capacity.

Do not hold source work merely because one PR is in remote CI. Reduce launch
rate before the machine is saturated; do not modify or kill unrelated user
processes.

Native work is centralized: one native gate at a time, `--jobs=1`, a fresh
three-sample CPU mean below 85 percent, and at least 6 GiB free RAM immediately
before launch. A native gate temporarily pauses new model launches when its
real load consumes the safe headroom; it does not justify activating idle PMs.

## Continuous PR conveyor

Every implementation packet follows this order:

1. Create an isolated, bounded worktree from the current local stack tip.
2. Implement and verify there. Preserve unrelated work and bind evidence to
   the final source bytes.
3. Commit the coherent packet and preserve a recovery ref when forward-porting
   worker output.
4. Push the verified branch and open a PR whose base is the immediately
   preceding branch in the live stack.
5. Register it with `gh stack link <active-stack> <pr>`; run
   `gh stack checkout <pr>` and verify remote head, base, and managed-stack
   membership with `gh stack view --json`.
6. Fast-forward the accepted branch into local `master`. This is local
   integration, not a remote merge, and does not wait for the remote PR merge.
7. Confirm preservation, remove the clean inactive worktree normally, and
   retire only its verified private build output.
8. Record the PR, commit, recovery ref, checks, local integration, retired path,
   and next packet in `docs/SESSION_HANDOFF.md`.

Never merge a GitHub PR, enable auto-merge, push local `master` to remote
`master`, or retarget a stack leaf to remote `master` without a new explicit
request. Publishing stacked PRs is mandatory; local integration never replaces
publication.

The conveyor must stay filled: Fable can architect later packets while workers
implement ready packets, one native gate verifies a finished packet, and the
VP publishes another. Do not serialize the entire organization behind remote
CI or one broad aggregate replay when proportionate focused evidence is enough.

The operating target is one genuine, reviewable PR every 5–10 minutes. This is
a pace diagnostic, not permission for no-op packets. At ten minutes without a
PR, the VP must record the concrete
stage counts (architecting, implementing, correcting, gating, publish-ready),
identify the bottleneck, and act on the smallest genuine publishable leaf.
Large gates may legitimately exceed ten minutes, but their active process and
progress must be visible while source workers continue elsewhere when safe.

Worker churn is the controlling health signal. A PM that is not delegating,
reviewing a live worker, holding an authorized gate, or handing off accepted
bytes is inactive and must drain. Five PMs with zero live workers is an
immediate orchestration failure: stop adding review/planning layers, launch
ready worker contracts across the existing PM lanes, and publish the smallest
accepted leaf. Never let optional review, speculative architecture, or a
problem with no concrete evidence block source workers or a green PR.

## Managed-stack recovery

Read the active stack number from `docs/STACK.md` and the latest handoff. If
local `gh-stack` tracking is shorter than the remote stack, back up the common
Git directory's `gh-stack` file, snapshot branch refs, verify the stack number,
and run only:

```text
gh stack unstack <active-stack> --local
gh stack checkout <latest-pr>
```

The `--local` flag is mandatory. Verify branch refs are unchanged, other stacks
remain tracked, and the refreshed stack contains the new PR. Never use remote
unstacking or `gh stack sync` against deliberately integrated local `master`.
If GitHub rejects a stack-size increase, start a managed continuation from the
last stack branch and record the dependency between stack numbers.

## Worktree and disk safety

- Resolve the exact registered worktree path and verify HEAD/status before
  retirement. Never recursively delete an unresolved path or workspace root.
- Verify no worker, compiler, test, or session uses the checkout.
- Preserve every committed result in the PR/local integration and keep an
  explicit recovery ref for cherry-picked worker commits.
- Use ordinary `git worktree remove` without force for clean inactive trees.
  Dirty or unfinished work must be preserved and investigated.
- Use the build tool's cleanup for verified private caches. Preserve shared
  caches, active logs, recovery archives, refs, and unrelated user files.

## Compaction-proof live handoff

`docs/SESSION_HANDOFF.md` is the single live state ledger and is intentionally
local/untracked. Update it after every material worker launch/completion,
native-gate transition, PR publication, local integration, worktree retirement,
rate limit, or blocker. Each live lane record must include:

- Fable PM identity and vertical;
- implementation provider/model/session/PIDs and exact CLI surface;
- worktree, branch, base/HEAD, owned/no-touch paths;
- stage, last advancement time, checks and immutable evidence;
- publication/integration/retirement state and exact next action.

Before yielding the main task, ensure the handoff names the next executable
action. The VP task remains alive while the port is incomplete; an interruption
is recovered through the restart sequence at the top of this file.
