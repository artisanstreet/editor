# Native port execution plan

## Outcome

Deliver the native Artisan editor as a coherent vertical system through a
continuous managed stack of small PRs. Keep architecture ahead of implementation
without allowing architecture work to replace implementation throughput.

The detailed product contracts live in `docs/plans/` and the live checkpoint
lives in `docs/SESSION_HANDOFF.md`. This plan defines how Fable architects turn
those contracts into a sustained external implementation-worker queue.

## System streams

Fable PM/architect lanes should be organized by independent ownership streams,
not arbitrary phases or one-PM-per-file bureaucracy:

1. **Engine persistence:** E2 durable progress and paired terminal settlement,
   then E3 startup reconciliation. Database repository ownership makes these
   packets serial within the stream.
2. **Native engine ownership:** E4 validated configuration, admission, process
   custody, protocol lifecycle, shutdown, and real-engine proof. Interfaces can
   be architected alongside persistence; implementation observes dependency
   gates in `docs/plans/NATIVE_ENGINE_FIRST_WORKFLOW.md`.
3. **Conversation delivery:** E5 bounded projection reads, subscriptions,
   post-commit delivery, and replay behavior. Proceed once E2 public shapes are
   frozen; coordinate shared database exports serially.
4. **Forge assembly:** E6 process composition and custody. This integrates E1–E5
   rather than inventing a parallel runtime.
5. **Client projection:** E7 transport/client projection and native rendering,
   building on accepted delivery interfaces.
6. **End-to-end proof:** E8 fixture-driven and real-engine workflows, failure,
   interruption, restart, and recovery evidence.
7. **Portable packaging:** deterministic archive, clean-host layout, launcher,
   manifest, asset/dependency completeness, and installation proof.
8. **Native shell and UI:** shell composition and remaining inventory-backed
   components/interactions, integrated through existing UI/frontend modules.

Shared `lib.rs`, module BUILD files, root `BUILD.bazel`, Cargo manifests, locks,
and database repository exports are VP integration surfaces. Workers should own
behavioral source/tests; Fable must explicitly schedule shared registration so
parallel source packets do not collide.

## Fifty-worker queue

The target queue contains up to 50 implementation-ready worker packets. Fable
maintains it as a dependency graph, not a flat wish list. A packet enters READY
only when all fields below are concrete:

- vertical and user-visible/system outcome;
- exact base commit and prerequisites;
- exact owned and no-touch paths;
- frozen public interfaces and invariants;
- focused acceptance tests and permitted commands;
- native-gate needs and estimated cost;
- conflict surfaces and VP-owned integration edits;
- completion artifact: commit, diff scope, evidence, and handoff format.

READY packets should normally be one coherent PR each. Split work where pieces
are independently useful and testable; combine pieces whose intermediate state
would be fake, broken, or only administrative. Documentation-only/no-op packets
do not count toward the 50-worker target unless documentation is itself the
required product outcome.

Each Fable lane keeps multiple READY packets and supervises four to eight
implementation workers during a healthy source wave. Sol selects the current
model tier from `.agents/MODELS.md` and admits workers in small batches across
disjoint worktrees, resamples machine pressure, and refills as workers finish.
The backlog may contain 50 while fewer are live due dependencies or host load;
the handoff must distinguish READY, WORKING, GATING, PUBLISHABLE, BLOCKED, and
DONE counts.

## Conveyor scheduling

At all times, try to keep independent stages occupied:

- Fable freezes downstream contracts.
- External Fable/Opus/Muse/Luna workers implement READY source packets under
  the strict model cascade.
- One native gate verifies the highest-priority finished packet.
- Sol reviews/publishes a different already-green packet.
- Completed worktrees are locally integrated and retired immediately.

Priority is dependency-unblocking first, then the smallest genuine leaf that
keeps the stack moving. Do not wait for remote CI before beginning the next
local packet. Do not let a broad replay stop source-only work in disjoint lanes
when resource headroom is safe.

The target publication pace is one genuine PR every 5–10 minutes. At ten
minutes without publication, Sol records READY/WORKING/CORRECTING/GATING/
PUBLISHABLE counts and fixes the smallest actual bottleneck. A long native gate
may exceed ten minutes only while its process/progress is visible and safe
source-only workers continue elsewhere. Do not create no-op PRs to fake pace.

Worker churn is mandatory. A PM without a live delegated worker, active review
of returned bytes, authorized gate, or publication handoff is inactive. Five
PMs with zero workers is an immediate pipeline failure: stop adding planners or
reviewers and launch READY contracts through existing lanes. Do not let
speculative issues or optional review layers deadlock accepted source.

## Fable-to-worker packet cycle

1. Fable reads the applicable plan and nearby production/tests.
2. Fable freezes the bounded contract and validates it against the current
   local stack tip.
3. The lane provisions one isolated worktree per independent implementation
   worker.
4. Sol selects Fable, Opus, fresh-available Muse, or external Codex CLI Luna
   using `.agents/MODELS.md` and the canonical provider-status file.
5. Fable launches the external worker process and records provider, exact
   model, CLI, session/PIDs, and live state. Codex collaboration/subagent APIs
   are never part of this cycle.
6. The worker implements, tests, reviews, and commits.
7. Fable reads the actual final diff/evidence. It accepts, requests a bounded
   same-session correction, or rejects with concrete reasons.
8. Fable returns a publication-ready receipt to Sol.
9. Sol performs shared integration, proportional final gates, stack publication,
   local-master fast-forward, retirement, and queue refill.

An architect-only result is not completion. Unless the vertical is explicitly
architecture-only, Fable remains responsible until at least one Muse packet is
working or a concrete dependency/resource gate is recorded.

## Stall and compaction recovery

Every material transition is written to `docs/SESSION_HANDOFF.md`. If the PR
cadence stops, use current evidence to classify the bottleneck:

- no READY packets: Fable architecture starvation;
- READY but no workers: admission/refill failure;
- workers without advancement: provider, permission, or contract failure;
- many finished workers but no gate: native-gate scheduling failure;
- green commits without PRs: VP publication failure;
- published PRs with retained trees: retirement/disk failure.

Fix the identified stage instead of adding more managers. After compaction,
follow the restart sequence in `AGENTS.md`, reconstruct these counts, resume
valid sessions, and continue from the exact next action in the handoff.

Muse availability is never rediscovered independently by PMs. The canonical
`.agents/PROVIDER_STATUS.md` caches the VP's result for one hour. Fresh status
is accepted as fact; stale status is escalated to the VP for one probe.
