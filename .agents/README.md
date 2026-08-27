# Agent operations index

This directory contains the stable operating material for the Artisan editor
native port.

- `../AGENTS.md` is the authoritative hierarchy, delivery workflow, resource
  policy, PR conveyor, and compaction protocol.
- `PLAN.md` is the durable port decomposition and worker-queue policy.
- `MODELS.md` pins model roles, invocation policy, and failure behavior.
- `launch-codex-luna.ps1` is the fail-closed external Luna worker launcher. It
  disables Fast mode and both Codex multi-agent feature paths.
- `PROVIDER_STATUS.md` is the VP-owned provider health cache. PMs must accept a
  Muse result less than one hour old and must never probe OpenCode themselves.
- `../docs/SESSION_HANDOFF.md` is the local, untracked live state ledger.
- `../docs/STACK.md` records the managed GitHub stack.

Static policy belongs here or in `AGENTS.md`. Volatile PIDs, sessions, hashes,
PR numbers, worktrees, resource readings, and next commands belong only in the
handoff so compaction recovery has one current source of truth. The narrow
exception is `PROVIDER_STATUS.md`: it stores only provider/cascade health,
timestamps, one-hour freshness, and the next permitted Muse probe so every PM
shares one result instead of wasting capacity on duplicate probes.
