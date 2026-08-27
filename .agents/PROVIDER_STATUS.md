# External model provider status

This is the single VP-owned health cache for model admission. Read this file
from the canonical checkout, not from a stale worker worktree.

Last updated: **2026-08-27T02:07:48+02:00**

## Current cascade state

| Tier | Model / surface | Status | Checked at | Valid until | Evidence / action |
| --- | --- | --- | --- | --- | --- |
| 1 | Claude Fable High via `claude --model fable --effort high` | `AVAILABLE_PRIMARY` | 2026-08-27T01:23:29+02:00 | Until actual Claude cap/rate-limit evidence | Consume Fable allocation first for PM and separate worker sessions. |
| 2 | Claude Opus 5 via `claude --model opus --effort high` | `STANDBY` | Not probed | After Fable depletion | Do not consume until Fable reports a concrete cap/rate limit. Startup must resolve to `claude-opus-5`. |
| 3 | Muse Spark via `opencode2` | `RATE_LIMITED` | 2026-08-27T01:22:54+02:00 | 2026-08-27T02:22:54+02:00 | Exact Muse sessions returned HTTP 429. No PM may probe or launch while this result is fresh. |
| 4 | GPT-5.6 Luna Max via external `codex exec` | `STANDBY` | Not probed | After Claude depletion when fresh Muse is not available | Fast mode and both multi-agent features must be disabled. Never use Codex subagent APIs. |

Current new-worker tier: **Claude Fable High**.

Next eligible Muse probe: **not before 2026-08-27T02:22:54+02:00**.

## Mandatory freshness rule

- A Muse result is fresh for 60 minutes from `Checked at`.
- Every PM accepts a fresh result exactly as written. It does not run
  `opencode2 models`, provider discovery, a tiny inference, or a real worker to
  retest it.
- If the result is stale, the PM reports `MUSE_STATUS_STALE` to the VP and
  waits. Only the VP probes and updates this file.
- A provider probe is not an implementation worker and never counts as one.

## VP-only Muse probe protocol

At most once per rolling hour, and only after `Next eligible Muse probe`:

1. Prove no Muse worker/probe is live.
2. Use the verified OpenCode binary recorded in the handoff.
3. Run one standalone, no-tools request from an isolated scratch directory:

   ```powershell
   opencode2 run --standalone --format json `
     --model opencode/muse-spark-1.2-contributor-free `
     "Do not use tools. Reply with exactly MUSE_PROBE_OK and nothing else."
   ```

4. Record `AVAILABLE` only for a natural zero exit with exact
   `MUSE_PROBE_OK`. Record `RATE_LIMITED` for HTTP 429, `UNAVAILABLE` for
   no-route/model-unavailable, and `UNKNOWN` for everything else.
5. Set `Checked at`, `Valid until`, and `Next eligible Muse probe` to preserve
   the one-hour cache. Do not retry an ambiguous or failed probe inside the
   hour.

Muse never preempts remaining Claude allocation. When Muse later becomes
`AVAILABLE`, use it only after both Fable and Opus are concretely depleted. If
Muse is still unavailable then, launch Luna through the exact external Codex
CLI policy in `MODELS.md`.
