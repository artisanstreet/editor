# Agent operations index

This directory contains the stable operating material for the Artisan editor
native port.

- `../AGENTS.md` is the authoritative hierarchy, delivery workflow, resource
  policy, PR conveyor, and compaction protocol.
- `PLAN.md` is the durable port decomposition and worker-queue policy.
- `MODELS.md` pins model roles, invocation policy, and failure behavior.
- `../docs/SESSION_HANDOFF.md` is the local, untracked live state ledger.
- `../docs/STACK.md` records the managed GitHub stack.

Static policy belongs here or in `AGENTS.md`. Volatile PIDs, sessions, hashes,
PR numbers, worktrees, resource readings, and next commands belong only in the
handoff so compaction recovery has one current source of truth.
