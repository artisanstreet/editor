# Vault credential artifacts v1 (proposal for Core owner, via root)

Goal: sync the auth the user already did — subscription OAuth and sessions —
not just API keys. No fake OAuth, no invented provider APIs. Every artifact
is a verbatim encrypted copy of what the vendor CLI wrote locally; Forge
re-materializes it into its engine home and verifies with the vendor's own
status probe.

## 1. What the engines actually store (observed in this repo)

- Claude (`crates/forge-engines/src/account.rs`, `engines.rs`): device flow
  `auth login` (accepts pasted code), probe `auth status` → JSON
  `{loggedIn, authMethod, apiProvider, email, subscriptionType}` (snapshot
  test vectors in `account.rs`). Config dir pinned to the engine home via
  `CLAUDE_CONFIG_DIR` → `<home>/.claude`. Credentials are vendor-managed
  OAuth tokens (subscription, e.g. `subscriptionType: "max"`), NOT API keys.
- Codex: `login --device-auth` (link + `XXXX-XXXX` device code, no pasted
  code), probe `login status` → `Logged in using ChatGPT` /
  `Not logged in`. Config via `CODEX_HOME` → `<home>/.codex`.
  ChatGPT-subscription OAuth, not an API key.
- OpenCode (`account.rs`, `usage.rs`, `install.rs`): `auth login
  --standalone`, `integration.list`, `auth logout`. Connections carry
  `{type: credential, id, method: oauth}`; credential values are
  `{type: "key", key}` (API key, e.g. `opencode-go`) or
  `{type: "oauth", access, expires, metadata: {orgID}}` (Console/Go
  subscriptions). Storage observed at `<home>/data/opencode/auth.json`
  (XDG DATA layout; Artisan builds use `OPENCODE_HOME=<home>`). Signed-out
  still runs free models.
- claude.ai mode (`forge-engines/src/claudeai/session.rs`): cookie session
  (`sessionKey`/`sessionKeyV3` + Cloudflare clearance bound to the browser
  user agent), stored at `<home>/.claude-ai/session.json`, mode `0600`,
  rotated from `Set-Cookie` on every response, `NeedsRenewal` when
  Cloudflare demands a browser check. A session artifact, not a key.
- Engine homes: `<data>/engines/<engine>/home` (versioned installs beside
  them). `materialize()` already writes prompt/skill links per launch.

API-key-only vault cannot represent any of the above except the
`opencode-go` key case. The artifact below carries the real files/values.

## 2. Envelope v1 (Core stores, Forge verifies, Relay never sees)

JSON, strict requests (unknown fields → 400), forward-compatible reads.
`accountId` is server-derived from the session/machine auth — clients never
claim it except echoing it inside signed proof strings where the server
re-derives and compares (Core `contracts.md` §1).

```jsonc
{
  "v": 1,                        // envelope version; bump on shape change
  "artifactId": "art_01K…",      // ULID, server-allocated
  "engine": "claude|codex|opencode|claudeai",
  "kind": "oauth-device|api-key|session-cookies|opencode-credential",
  "label": "operator-given, e.g. server-forge",
  "revision": 3,                 // increments on rotation/re-export
  "createdAt": "2026-10-26T20:00:00.000Z",
  "updatedAt": "2026-10-26T21:00:00.000Z",
  "expiresAt": null,             //.oauth expiry when known, else null
  "alg": "xchacha20poly1305-v1", // AEAD; per-artifact random 24B nonce
  "keyId": "kek-2026-10-a",      // wrapping account KEK id (rotation)
  "nonce": "<base64url 24B>",
  "ciphertext": "<base64url>",   // sealed plaintext bundle (§3), ≤64 KiB
  "contentHash": "<hex sha256 of plaintext bundle bytes>",
  "files": [                     // materialize plan, discovered at export
    {"path": ".claude-ai/session.json", "mode": "0600"}
  ],
  "probe": ["auth", "status"],   // vendor status command to verify
  "assignments": ["mch_01K…"]    // machines allowed to receive (server enforced)
}
```

Rules: ciphertext ≤64 KiB (all observed bundles are KBs); `path` is relative
to the engine home and may not escape (`..` rejected); `mode` is `0600`
(files) / `0700` (dirs); logs print names/revisions/hashes only — never
tokens, cookies, or key material (`Debug` redacts, as `Session` already does).

## 3. Plaintext bundles (decrypted only on assigned Forges, memory-first)

Each bundle is JSON `{"bundle": <name>, "v": 1, "files": {relPath: base64}}`
plus parsed metadata for display. Forge validates the bundle against the
engine's adapter before writing anything.

- `claude.oauth.v1`: `files` = verbatim credential file(s) discovered under
  `<home>/.claude` at export (exact vendor filenames are export-discovered,
  not hardcoded here); `meta` = `{email?, subscriptionType?}` from
  `auth status`. Materialize: write `0600`, run `auth status`, require
  `loggedIn: true`.
- `codex.oauth.v1`: `files` = verbatim file(s) under `<home>/.codex`;
  `meta` = `{account?}` from `login status`. Materialize + `login status`
  must show signed in.
- `opencode.credential.v1`: `files` = verbatim `data/opencode/auth.json`
  (whole file; preserves every integration). `meta` = connection inventory
  `{integration, name, method}` with credential ids only. Materialize +
  `integration.list` must show the connections. Handles both `key` and
  `oauth` (with `expires`/`orgID`) — the actual tokens move, nothing is
  re-minted.
- `claudeai.session.v1`: `files` = `{".claude-ai/session.json": bytes}`;
  `meta` = `{account?, needsRenewal?}`. Materialize `0600` + state check.
  Rotation note: cookies rotate on use; Forge re-exports a new `revision`
  when its on-disk session changes (same `artifactId`), others pull.
- `apikey.v1` (fallback only): `{provider, envVar, value}` for users who
  prefer keys. Materialize as process env for launches only — never written
  to shell history or disk outside the sealed store.

Export = read the files the vendor CLI wrote + run the vendor probe (proves
the bundle works before upload). Import = write `0600`/`0700` + run the same
probe (proves the destination works after). No new OAuth flows, no browser
automation, no token minting on the Forge.

## 4. Core implementation (Reylo, Postgres via Hyperdrive — no D1)

- Migrations: `db/vault_*.sql` (Reylo: ALL queries in `.sql` files):
  `vault_artifacts(artifact_id PK, account_id, engine, kind, label, revision,
  alg, key_id, nonce, ciphertext, content_hash, files JSONB, probe JSONB,
  expires_at, created_at, updated_at)`; `vault_assignments(artifact_id,
  machine_id, assigned_at, PK(artifact_id, machine_id))` with FK to machines;
  `vault_applied(machine_id, artifact_id, revision, ok, at)` for
  applied-revision reports. Content-addressed dedup optional (unique on
  `(account_id, content_hash)`).
- Effect Schema is authoritative; `POST` bodies deny unknown fields; `GET`
  responses are forward-compatible. Export OpenAPI; fixtures under
  `fixtures/v1/vault.*.json` (additive, v1-tolerant readers).
- Endpoints (session auth for account scope; machine proof/JWT `aud=ast-core`
  + `machine:sync` for machine scope):
  `POST /v1/vault/artifacts` (encrypt with account KEK, store),
  `GET /v1/vault/artifacts?engine=` (account view, ciphertext included for
  owners), `GET /v1/vault/assignments` (machine: only its assigned artifacts,
  decrypt-on-release over mutually-auth TLS),
  `POST /v1/vault/assignments {artifactId, machineId}` (owner),
  `DELETE /v1/vault/artifacts/{id}` / `DELETE …/assignments`,
  `POST /v1/vault/applied {artifactId, revision, ok}` (machine receipt).
- Encryption: per-artifact random DEK → XChaCha20-Poly1305 (24B nonce);
  DEK wrapped by the account KEK (KMS/manual rotation via `keyId`); KEK never
  leaves Core; Postgres holds ciphertext only. Small blobs stay in Postgres
  (storage boundary small); no Relay involvement.
- Assignment enforcement server-side: release decrypts ONLY for machines in
  `vault_assignments`; every release logged as (account, artifact, revision,
  machine, at) without secret bytes.

## 5. Forge implementation (this repo, `forge-engines` + `forge-http`)

- `export`: per engine, collect `files` from the engine home using the same
  `host_spec().config` roots the launcher already uses; run the vendor probe
  (`account.rs::check`); bundle + sha256; POST sealed artifact (Forge never
  sees the KEK — TLS protects the plaintext in flight to Core, Core encrypts
  at rest).
- `materialize`: verify `contentHash` after decrypt, write files with
  `0600`/`0700` (reuse `claudeai/session.rs::restrict`), run the vendor probe
  again, report `vault/applied`. Keep last-good cache: on Core-unreachable,
  keep serving existing engine homes (offline works).
- Rotation: a watcher on the session/auth files re-exports new revisions
  (cookies rotate); conflicts resolve by revision (higher wins, per-machine
  applied receipts visible in the UI).
- Tests (meaningful, no fakes): roundtrip export→seal→open→materialize→probe
  with fixture CLIs (as `account.rs` tests already do with throwaway homes);
  tamper (bit-flip ciphertext → AEAD failure, no write); wrong-machine release
  refused; probe-failure blocks `applied: ok`; redaction (logs contain no
  token bytes).

## 6. Editor (this repo, `editor-core` + settings)

Vault UI shows names/revisions/assignments/probe state only — never secret
bytes. Assign/unassign calls Core; per-machine `applied` receipts show
whether the destination engine actually verified. No secret leaves the Forge
except the TLS-sealed upload to Core and the assigned release back.

## 7. Open verification before v1 freeze

1. Confirm exact vendor credential filenames by probing installed CLIs
   (opencode path is confirmed; claude/codex filenames are export-discovered,
   not assumed — first `export` on a signed-in machine records them).
2. Core publishes `fixtures/v1/vault.*.json` + OpenAPI; Rust verifies one
   generated vector (hash + probe shape) before freeze.
3. Noise-library decision (Relay) does not block this: vault uses
   XChaCha20-Poly1305 AEAD at rest + TLS in flight, independent of the tunnel
   cipher.
