# Model roles and external-worker cascade

Model transitions are explicit, evidence-backed controller decisions. Never
silently substitute a model because a queue is slow, and never route a worker
through the root Codex collaboration/subagent API.

## Non-negotiable dispatch boundary

- PMs and implementation workers are external CLI processes with recorded
  executable, model, session, PID lineage, worktree, contract, and logs.
- Do not call `spawn_agent`, `followup_task`, model overrides on a collaboration
  call, or equivalent Codex subagent APIs for this repository's workforce.
- In particular, GPT-5.6 Luna must run through `codex exec`. A subagent-model
  override does not create Luna; it can replicate the root GPT-5.6 Sol/XHigh
  task and burn the wrong usage pool.
- PM and implementation responsibilities remain separate sessions even when
  both sessions use Claude Fable. A PM reviews product bytes; it does not author
  them in its own session.
- Existing healthy workers finish naturally during a tier transition. New
  sessions use the newly selected tier; never kill work merely to change model.

## Strict capacity order

Use new implementation sessions in this order:

1. Claude Fable High while its current 20x allocation remains usable.
2. Claude Opus 5 after Fable reports an actual usage exhaustion, rate limit, or
   session cap.
3. Muse Spark after Claude capacity is depleted, but only when the canonical
   `.agents/PROVIDER_STATUS.md` contains a VP-owned `AVAILABLE` result less
   than 60 minutes old.
4. GPT-5.6 Luna Max through external Codex CLI when Claude is depleted and
   fresh Muse status is not `AVAILABLE`.

An actual provider response, quota message, or session-cap message is required
to advance tiers. Record the transition in the provider-status file and live
handoff. Do not bounce repeatedly between tiers or run retry storms.

## Sol — VP/controller

- Runtime: this root Codex task, normally `gpt-5.6-sol`.
- Role: global orchestration, live accounting, integration review, native-gate
  authorization, managed-stack publication, local-master integration, disk
  retirement, provider-status custody, and handoff maintenance.
- Must not: routinely author implementation packets, count itself as a worker,
  or use its collaboration tools to create the external workforce.

## Tier 1 — Claude Fable High

- CLI: `claude`.
- Exact selection: `--model fable --effort high`.
- Uses: PM/architect sessions and separate implementation-worker sessions.
- This user-authorized policy supersedes the old ban on Fable implementation;
  product edits are allowed only in bounded worker sessions, never in the PM
  session supervising them.
- Prefer persistent/resumable PM sessions by vertical. Implementation workers
  remain one finite packet per isolated worktree.

Representative noninteractive launch:

```powershell
claude -p --model fable --effort high --permission-mode acceptEdits `
  --output-format stream-json --verbose
```

Pass the prompt over stdin or a final unambiguous argument. Add exact worktree
and log directories with `--add-dir`, plus a comma-separated tool allowlist
when required. Never set `--fallback-model`; the VP owns tier transitions.

## Tier 2 — Claude Opus 5

- CLI: `claude`.
- Exact selection: `--model opus --effort high`.
- Startup telemetry must resolve the alias to `claude-opus-5`; otherwise hold
  without inference or edits.
- Uses the same separate PM/worker contracts, worktree isolation, evidence, and
  correction discipline as Fable.

Representative launch:

```powershell
claude -p --model opus --effort high --permission-mode acceptEdits `
  --output-format stream-json --verbose
```

Use Opus only after concrete Fable depletion/cap evidence. When Opus or the
shared Claude allocation is exhausted, advance to the fresh Muse/Luna decision
instead of probing more Claude aliases.

## Tier 3 — Muse Spark when fresh status is available

- CLI: the verified `opencode2` binary recorded in the handoff.
- Exact model: `opencode/muse-spark-1.2-contributor-free`.
- Muse is eligible only after Claude depletion and only while canonical
  `.agents/PROVIDER_STATUS.md` says `AVAILABLE` from a successful probe less
  than 60 minutes old.

Representative worker launch:

```powershell
opencode2 run --standalone --auto --format json --thinking `
  --model opencode/muse-spark-1.2-contributor-free `
  --file <contract-path> "Implement exactly the attached bounded packet."
```

Do not append quality suffixes, change providers, or create a fresh session for
a correction when the existing session remains valid.

### Muse probe ownership and one-hour cache

- Only the VP probes Muse. PMs and workers never run `opencode2 models`, model
  discovery, or inference probes.
- The result in canonical `.agents/PROVIDER_STATUS.md` is authoritative for 60
  minutes from `checked_at`.
- If the entry is fresh, every PM accepts it. If stale, a PM reports `STALE` to
  the VP and waits; it does not probe.
- The VP runs at most one isolated, no-tools probe per rolling hour. Success
  requires a natural zero exit and exact `MUSE_PROBE_OK`; HTTP 429 records
  `RATE_LIMITED`; no-route/unavailable records `UNAVAILABLE`; any ambiguous
  result records `UNKNOWN` and is not usable.
- A failed probe is not retried until `next_probe_not_before`.

Representative VP-only probe:

```powershell
opencode2 run --standalone --format json `
  --model opencode/muse-spark-1.2-contributor-free `
  "Do not use tools. Reply with exactly MUSE_PROBE_OK and nothing else."
```

## Tier 4 — GPT-5.6 Luna Max through Codex CLI only

- CLI: `codex exec`; never the collaboration/subagent API.
- Exact model: `gpt-5.6-luna`.
- Reasoning: `model_reasoning_effort="max"`.
- Service: Standard/non-fast. Ignore user config and explicitly disable
  `fast_mode`; never pass `service_tier="fast"` or use `/fast on`.
- Descendants: explicitly disable `multi_agent` and `multi_agent_v2`. Never use
  Ultra reasoning because Ultra is a subagent workflow.
- Sandbox: `workspace-write`; approval policy `never` for bounded
  non-interactive workers. Do not use `--yolo`.
- Preserve sessions for bounded corrections; do not use `--ephemeral`.

Canonical PowerShell launch shape:

```powershell
$contract = Get-Content -Raw -LiteralPath <contract-path>
$contract | codex exec `
  --ignore-user-config `
  --disable fast_mode `
  --disable multi_agent `
  --disable multi_agent_v2 `
  --model gpt-5.6-luna `
  --config 'model_reasoning_effort="max"' `
  --config 'approval_policy="never"' `
  --sandbox workspace-write `
  --cd <worktree> `
  --json `
  -
```

The launcher resolves and hashes the `codex` executable, records
`codex --version`, captures stdout/stderr and session ID, and verifies from
startup telemetry that the model is Luna, effort is Max, Fast mode is off, and
multi-agent tools are absent. If the installed CLI rejects `max` or the model
slug, hold and report it; never silently use XHigh, Sol, Fast, or Ultra.

For a correction, use `codex exec resume <SESSION_ID>` with the same model,
reasoning, sandbox, Fast-mode, and multi-agent overrides before the bounded
follow-up prompt.

## Accounting and receipts

- Fable or Opus PM process/session: one PM, not a worker.
- Separate Fable, Opus, Muse, or Luna implementation process actively editing
  a bounded packet: one worker.
- Native test/build process verifying a packet: one gate, not a worker.
- Root Sol, idle servers, blocked/stalled sessions, plans, reviews, worktrees,
  provider probes, and completed agents: zero working workers.
- Every receipt names the provider, exact model, executable/version/hash,
  session/PIDs, tier-selection evidence, worktree/paths, checks, and uncertainty.
