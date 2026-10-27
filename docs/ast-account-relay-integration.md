# AST account + Relay integration (Editor repo)

Status: implementation in progress. Root is router. Relay `docs/protocol.md` v1 (2026-10-26) is the interim wire contract; Core `docs/contracts.md` does not exist yet (Core repo absent). This file is the Editor-side map, decisions, and journey tracking so Core/Relay can align without us editing their repos.

## 1. Exact Rust integration points (read, not yet changed except additively)

- `crates/artisan-api/src/request.rs` — `State`, `Account` (OS user/host today, not AST identity), `PairRequest/Response`, `ApiError`.
- `crates/artisan-api/src/ids.rs` — `DeviceId`, `ThreadId`, etc. AST `AccountId`/`MachineId` are new, validated like `idCharset` in Relay `schema.ts`.
- `crates/artisan-api/src/tls.rs` + `crates/forge-http/src/tls.rs` — mutual TLS, pinned fingerprint. Direct paths keep this untouched.
- `crates/forge-store/src/pairing.rs` + `admin.rs` — `pair_device` (consume code + authorize key in one write), `is_authorized` (memory-cached), `add_pair_code`. Account enrollment reuses the same single-decision shape, new state, new table/file (not same rows).
- `crates/forge-store/src/schema.rs` — append-only migrations. No AST migration yet; machine keys live in `secrets` (existing) to avoid churn.
- `crates/forge-http/src/routes.rs::authorize` — device-cert gate. Extension point: allow valid Core grant (Bearer + PoP-bound) without weakening device path.
- `crates/forge-http/src/lib.rs::invite/invite_offline` — 8-char code, 10-min TTL, `hash_code` (upper-normalised SHA-256 hex). Enrollment mirrors TTL/single-use/rate-limit with explicit local confirm.
- `crates/forge-http/src/gateway.rs` — loopback MCP only, not the Relay path.
- `crates/ae/src/forge.rs::pair` + `main.rs::Command::Pair` — `ae pair [--json]`, waits for device row. `ae account enroll` is new, same UX shape.
- `crates/editor-core/src/hosts.rs::HostConfig` — `{id,name,address,fingerprint,kind}`. Relay route is additive `Option<RelayRoute>` (serde default), old `hosts.json` still parses.
- `crates/editor-core/src/client.rs::ForgeClient` — direct HTTPS+mTLS. Relay path is a new `via_relay` channel handle reusing the same JSON DTOs; direct stays default.
- `crates/editor-core/src/connection.rs::follow` — SSE resume. Relay presence (`forge_online`) feeds the same `ConnectionStatus`, does not replace SSE framing.
- `crates/editor-settings/src/hosts.rs` — pairing UI. Account sign-in is a new section, not a change to existing rows.
- `crates/editor-core/src/screen.rs` — `artisan://` links. Sign-in handoff adds `artisan://signin?...` with safe binding, GET never consumes.

## 2. Contracts (from Relay `docs/protocol.md` + `src/*.ts`, frozen unless interop blocker)

- Grant JWT: Core-signed EdDSA (`kid`), `aud: ast-relay-v1`, TTL ≤300s, single-use `jti`, claims `{v:1,iss,aud,jti,iat,[nbf,]exp,account,machine,role:forge|client,scope,cnf:{jwk:OKP Ed25519},rev}`. Unknown top-level claims rejected at v=1 (strict request reads). Relay scopes: `relay:route`, `relay:pair`, `forge:exec`, `forge:sync`, `relay:admin` (admin cannot open tunnels). Forge re-verifies `forge:*` (opaque to Relay).
- Admission: `GET /v1/route/{account}/{machine}` → WS; `hello{grant}` ≤5s → `challenge{nonce32,timeout_ms:60000}` → `authenticate{jti,sig}` where `sig=Ed25519(cnf_sk,"ast-relay-auth-v1"||nonce||jti_utf8)` → `ready{role,account,machine,forge_online}`. Forge replacement needs fresh grant+PoP. Failures close 4401/4403/4408/4409/4001/4002, never log tokens.
- Inner E2E (browser-capable, NOT TLS-in-tunnel): long-term X25519+Ed25519 identity keys, fingerprints via Core account (distribution format open, proposal ` {machine,x25519_pk,ed25519_pk,fpr}` awaiting Core). Session: `ss1=X25519(eph_sk,recip_xpk)`, `ss2=X25519(sender_xsk,recip_xpk)`, `info="ast-relay-kem-v1"||0x00||account||0x00||machine||0x00||channel`, `wrap_key=HKDF-SHA256(ss1||ss2,salt=32×0x00,info,32)`, 32B session key, envelope `eph_pk||ChaCha20Poly1305(wrap_key,nonce=12×0x00,session_key,aad=info)` (80B). Data: `data_key=HKDF-SHA256(session_key,salt=32×0x00,"ast-relay-data-v1",32)`, frame `nonce24||XChaCha20Poly1305(data_key,nonce,pt,aad=channel_utf8||0x00||seq_be64)`. AAD binds channel+seq; Relay enforces strictly increasing seq per (channel,dir). Single fixed suite; version bump on change.
- Frames: client `hello/authenticate/open/data/close/pong`, server `challenge/ready/channel-opened/data/channel-closed/presence/error/ping`. Strict decode client→server (unknown fields error), forward-compatible reads server→client. Limits: body ≤256KiB b64 (~349525 chars), queue 256/channel, 64 clients, 10k revoked jtis, hello 5s, challenge 60s, ping 20s, pong timeout 60s, send buffer 1MiB (drop-newest), close at 4MiB.
- Revocation: `rev` epoch + Core-signed push `{v,grant(admin),account,min_rev,revoked[]}`; new auth immediate, live conns swept ≤30s, delivered messages not recalled. No attestation, no per-project scopes (paired account = full Forge exec per settled product decision), no replication semantics in v1.
- mTLS mapping: direct Forge↔Forge and loopback keep mTLS; Relay path uses JWT+PoP (caller) + per-channel session key (secrecy) + XChaCha AAD + seq (integrity/order) + rev epoch (revocation). Outer WSS is CF-terminated transport only.
- Cross-language: Relay `src/e2e.ts` byte layouts frozen; Rust must use vetted `x25519-dalek, ed25519-dalek, hkdf, sha2, chacha20poly1305, getrandom` and verify `contracts/fixtures/*.json` (none yet — Rust returns first fixed-seed vector before v1 freeze, see `crates/artisan-account/fixtures/`).

## 3. Decisions (current conversation overrides stale docs)

- DB: PostgreSQL through Hyperdrive remains the Core authority (Relay has no Hyperdrive/relational state by design). Editor/Forge treat Core as opaque HTTP; storage boundary small.
- Crypto: Ed25519 for grants/PoP (Core preference routed via root). No homemade crypto, no novel browser TLS stack. Browser path uses the vetted inner E2E channel (`@noble/*` ↔ Rust crates above); Relay contributor assessing browser specifics. Native keeps mTLS on direct routes.
- Trust: paired account grants full Forge execution. No per-project/read-only tiers. Token scopes distinguish `relay:route` vs `forge:exec` vs `forge:sync` (machine sync) vs `relay:pair`.
- Scope: no project replication in this build. No fake subscriptions or provider OAuth. Provider credentials encrypted at rest in Core, released only to assigned machines over mutually-authenticated TLS, held in Forge memory only, never logged.
- Enrollment: Forge keypair local; pending code short TTL, single-use, rate-limited; attacker guessing cannot enroll without explicit local Forge confirmation bound to exact account/key; authenticated account claim completes it.
- Sign-in: magic email links only (no passwords/passkeys/social). Link GET never consumes (mail scanners); cross-browser/email-on-phone flow requires explicit original-session approval + safe binding. TOTP optional + recovery codes; email alone must not bypass TOTP. Persistent revocable sessions.

## 4. Peer coordination status

- Core `01a17a5d-3ceb-…` / Relay `01a17a5d-3cfc-…`: `agent_message` from this thread returns “has no agent”; `thread_send`/`thread_read` return “no thread”. Peer threads unreachable from Editor Forge. Using root as router.
- Relay `docs/protocol.md` + `src/schema.ts|jwt.ts|e2e.ts|room.ts` read 2026-10-26. Open items needing Core/Rust confirmation are §7 of that doc; Editor answers Rust side with `artisan-account` + fixtures.
- Core `docs/contracts.md` absent (no `/home/sander/projects/artisanstreet/core`). Interim: Relay GrantClaims + JWKS shape is the Core contract proposal; Editor implements verifier with configurable `iss/aud/jwksUrl/maxTtl` and strict v1 checks, awaiting Core field confirmation (`iss`, JWKS URL, `aud` rename?, `relay:admin` push auth, fingerprint distribution format).

## 5. What actually works vs deployment prerequisites (updated as code lands)

- Works offline (no Core/Relay): direct `ae pair` + device-cert Forge API + SSE, existing tests.
- Works with new code, no deployment: `artisan-account` grant/PoP/E2E roundtrips + tamper/strictness/TTL tests; enrollment state machine (TTL/single-use/rate-limit/explicit confirm binding); Relay handshake framing against loopback stub; fixtures generate/verify.
- Needs Core deployed: account enrollment completion, JWKS fetch, grant issuance, config revision sync, provider vault release, email-link sign-in send/consume, machine discovery fingerprints.
- Needs Relay deployed: outbound WS `hello/challenge/authenticate`, presence, opaque `open/data/close` E2E routing, revocation sweep. Direct routes remain fallback; losing Relay never breaks direct.
- Browser sign-in/E2E: pending Relay browser assessment; native Rust E2E is the reference; do not ship homemade or TLS-in-WS fallback.

## 6. Files to add (additive, no behavior change to direct paths)

- `crates/artisan-account/` — GrantClaims/PoP/E2E/frames/enrollment/relay-config + fixtures + tests.
- `crates/forge-http/src/account_auth.rs` — Core grant re-verification (Bearer, scope, TTL, binding), device path first.
- `crates/forge-http/src/relay_connector.rs` — outbound WS + presence + opaque channel, no plaintext inspection.
- `crates/ae/src/account.rs` — `ae account enroll|confirm|status`, `ae relay status`.
- `crates/editor-core/src/account.rs` — sign-in handoff sessions, machine discovery, relay routing decision, config-revision + vault types (memory-only).
- This doc + `crates/artisan-account/fixtures/*.json` — cross-language vectors.
