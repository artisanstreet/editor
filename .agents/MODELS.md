# Model roles and invocation policy

Model roles are intentionally separated. Do not silently substitute one model
for another because a queue is slow, a provider fails, or a context compacts.

## Sol — VP/controller

- Role: global orchestration, live accounting, integration review, native-gate
  authorization, managed-stack publication, local-master integration, disk
  retirement, and handoff maintenance.
- Runtime: the root Sol-class Codex task, normally `gpt-5.6-sol` when a model is
  selectable.
- Must not: consume the implementation budget by routinely writing worker
  packets itself, create recursive Codex hierarchies, or count itself as a PM
  or implementation worker.

## Fable High — PM and system architect

- Exact selection: `claude --model fable --effort high`.
- Role: own a vertical, understand the existing system, freeze interfaces and
  dependencies, split PR-sized packets, supervise several Muse workers, review
  their real diffs/evidence, and return accepted commits to Sol.
- Fable is not the routine implementation model. It may write contracts,
  evidence, and lane-control artifacts outside product source. Product edits
  belong to Muse unless the user explicitly changes this policy.
- Prefer persistent/resumable Fable sessions for a vertical so correction and
  review retain architectural context. Record the session identifier and last
  advancement in the handoff.

Representative noninteractive launch:

```powershell
claude -p --model fable --effort high --permission-mode acceptEdits `
  --output-format stream-json "<bounded PM/architect contract>"
```

The concrete launcher may add exact allowed directories/tools and an evidence
path. It must not add a fallback model. If Fable is rate-limited or unavailable,
report it immediately, let already-running Muse workers finish, and preserve the
queue. Do not silently replace Fable with Sol, another Claude model, or Muse.

## Muse — implementation worker

- Exact model: `opencode/muse-spark-1.2-contributor-free`.
- Role: implement and test one finite contract in one isolated worktree.
- Required properties: standalone bounded session, exact file ownership,
  no descendants, no model fallback, evidence capture, final commit/hash report.

Representative launch:

```powershell
opencode2 run --standalone --auto --format json --thinking `
  --model opencode/muse-spark-1.2-contributor-free `
  --file <contract-path> "Implement exactly the attached bounded packet."
```

Use the repository's verified OpenCode binary/launcher recorded in the current
handoff. Do not append thinking/quality suffixes to the model identifier. If
Muse is rate-limited or returns a provider error, report it immediately and
preserve the session/worktree. Prefer a bounded same-session correction when
the provider and worker are still valid; never silently fall back to Sol,
Fable, Claude implementation, or another OpenCode model.

## Accounting

- `claude` Fable process/session: one PM/architect, not a worker.
- Advancing Muse implementation session: one worker.
- Native test/build gate actively verifying a packet: one gate, not a worker.
- Sol root, idle OpenCode servers, stalled/orphaned sessions, completed agents,
  reviewers, plans, worktrees, and queued contracts: zero working workers.
- Publish working counts from current process/session evidence, not historical
  agent lists.
