# Config + vault sync: requirements for Core (routed via root)

Editor/Forge implement against `core/src/contracts/v1.ts` +
`core/src/http.ts` (read 2026-10-27). The gaps below block autonomous
Forge sync. Nothing here invents wire shape — proposals mirror existing
Core patterns.

## R1 (blocking): machine-readable assignment discovery

Forge (machine proof auth) can download by id
(`GET /v1/engine-artifacts/:id/download`, `GET /v1/vault/credentials?connectionId=`),
but cannot enumerate what is assigned to it. List/assign endpoints are
session-scoped (`GET /v1/engine-artifacts`, `GET /v1/vault/connections`,
`POST …/assignments`), and `GET /v1/me` is session-only.

Proposed (mirrors existing shapes, strict requests, forward-compatible
reads):

```text
GET /v1/engine-artifacts/assignments        auth: machine
→ { "assignments": [{ "artifactId": art_…,
                      "engine": "claude|codex|opencode",
                      "kind": "oauth|api_key",
                      "exportedAt": "<rfc3339>",
                      "revoked": false }] }

GET /v1/vault/assignments                    auth: machine
→ { "assignments": [{ "connectionId": conn_…,
                      "provider": "anthropic|openai|openrouter|google" }] }
```

Questions for Core:
1. Accept these paths, or prefer extending an existing machine endpoint?
2. Revocation propagation: does a subsequent listing omit revoked/unassigned
   rows (Forge stops syncing, keeps last-good local, reports), or is an
   explicit revoked flag needed for audit?
3. Polling cadence guidance (Forge proposes 5 min + startup, ETag-gated
   where available).

Until R1 lands, Forge vault sync runs only for explicitly configured ids
(tests + `ae account vault-sync --artifact/--connection`), never by
invented discovery. No fake endpoints are coded.

## R2 (confirming): ETag / If-None-Match stability

`http.ts` already implements checksum invalidation on
`GET /v1/config/bundles/latest` (strong `ETag: "<contentHash>"`, 304 after
auth) and on the manifest route (immutable, 1-year private cache).
Forge implements the client side against exactly this: conditional fetch,
hash `bundle.contentHash` against the *manifest route bytes*, apply,
report. Please confirm this behavior is frozen for v1 (any change is a
contract bump, not a silent header tweak).

## R3 (decision): no `claudeai` artifact kind

Core `EngineArtifactEnvelope.engine` is `claude|codex|opencode`;
`kind` is `oauth|api_key`. Forge keeps `claude.ai` cookie sessions
local-only (they rotate on every response and need browser renewal —
pushing them through the vault would fight the renewal loop).
Recommendation: keep local-only for v1; do NOT add a session-cookies
kind until renewal handoff is designed. Confirm or correct.

## R4 (verify): credential release freshness signals

`GET /v1/vault/credentials` releases the secret to assigned machines.
Forge never overwrites a diverged local auth without a newer assigned
revision (freshness rule implemented + tested). Confirm: is there any
server-side "assigned revision" per (connection, machine) the Forge
should compare against, or is absence-from-discovery (R1) the only
revocation signal? Current implementation treats 404/410 on download as
revoked (stop syncing, keep last-good, report) — confirm that mapping.

## R5 (verify): EngineArtifact `expiresAt` semantics

`expiresAt: null` vs timestamp — does expiry mean "stop serving to the
machine" (Forge treats expired envelopes as not-applicable and keeps
last-good), or advisory display only? Forge currently treats a past
`expiresAt` as do-not-apply. Confirm.

## What Forge implements regardless (this repo)

- Conditional latest fetch, canonical-bytes hash verify, atomic apply,
  last-good state file, `applied`/`failed` report with detail; `304` and
  same-version+hash never reapply.
- Per-file `sha256` + `0600`/`0400` verify before any write; backup +
  rollback on failed materialize; KeptLocal on diverged local auth.
- Per-engine adapter report (up-to-date / updated / kept-local /
  needs-reauth) from the engines' own status probes — no invented engine
  auth APIs. Engines needing interactive sign-in are named, never silently
  papered over.
