# Native account client/controller API (proposal — Editor side owns this file)

Audience: root (routes), Vilde (implements the shared side). This file is the
exact seam between the native Editor account controller (owned here,
`editor-core/src/account.rs`) and the reusable account HTTP + Relay
transport (owned by Vilde, preferably `artisan-account::network`).
Names below are proposed so Vilde does not independently invent different
ones; root reconciles before either side renames.

## What already exists (reuse, do not duplicate)

- `artisan_account::enrollment::{MachineKeypair, normalise_code, fingerprint,
  enrollment_request_body, enrollment_confirm_body, sign_enrollment_proof,
  verify_enrollment_proof, PairStatus}` — machine keys, code normalisation,
  proof bodies. Matches Core contracts §2/§5.
- `artisan_account::pop::{generate_ephemeral, pop_sign, pop_verify}` —
  ephemeral `cnf` keys. (Editor derives the `cnf` JWK `x` via
  `MachineKeypair::generate().public()` + base64url; no new crypto.)
- `artisan_account::grant::{verify_claims, parse_and_verify_jwt, CnfJwk, …}` —
  verification lives with Relay/Forge; the native Editor never verifies
  grants, it only carries the JWT.
- `artisan_account::sync::{MachineRecord, RoutingDecision, RouteConditions,
  decide_route, ConfigRevision, VaultItem}` — routing decision inputs.
- `forge_http::core::CoreClient` — Forge-side machine client. Forge-specific;
  the GPUI Editor MUST NOT depend on the forge backend crate. The shared
  editor-side client belongs in `artisan-account::network` (new).

## Proposed shared surface: `artisan_account::network`

```rust
/// Session-side Core HTTP. Mirrors Core OpenAPI 1:1 (strict requests,
/// forward-compatible reads). No stores, no crypto beyond what
/// `artisan-account` already owns; the session token is opaque bytes.
pub struct CoreSessionClient { /* base_url, reqwest client */ }

impl CoreSessionClient {
    pub fn new(base_url: &str) -> Result<Self, NetworkError>;

    // Magic link (Core docs §6): requester-secret binding; GET never consumes.
    pub async fn magic_request(&self, email: &str, display_name: Option<&str>, device_name: &str)
        -> Result<MagicRequested, NetworkError>;   // { request_id, request_secret, expires_at }
    pub async fn magic_status(&self, request_id: &str, request_secret: &str)
        -> Result<LoginStatus, NetworkError>;      // { status: pending|email_approved|completed|expired, totp_required, email, expires_at }
    pub async fn magic_consume(&self, token: &str, request_secret: Option<&str>)
        -> Result<LoginStatusLite, NetworkError>;  // { request_id, status, totp_required }
    pub async fn magic_finalize(&self, request_id: &str, request_secret: &str,
                                totp_code: Option<&str>, recovery_code: Option<&str>)
        -> Result<FinalizedSession, NetworkError>; // { session_id, account_id, expires_at, totp_verified } + raw token via Set-Cookie capture

    // Session-authed (Bearer <raw session token>):
    pub async fn me(&self, session: &str) -> Result<AccountProfile, NetworkError>;
    pub async fn list_sessions(&self, session: &str) -> Result<Vec<SessionInfo>, NetworkError>;
    pub async fn revoke_current(&self, session: &str) -> Result<(), NetworkError>;
    pub async fn revoke_all_sessions(&self, session: &str) -> Result<(), NetworkError>;
    pub async fn totp_enroll(&self, session: &str) -> Result<TotpEnrollment, NetworkError>; // { otpauth_url, recovery_codes }
    pub async fn totp_verify(&self, session: &str, code: &str) -> Result<(), NetworkError>;
    pub async fn totp_disable(&self, session: &str, code: &str) -> Result<(), NetworkError>;
    pub async fn claim_enrollment(&self, session: &str, code: &str) -> Result<ClaimedEnrollment, NetworkError>; // { enrollment_id }
    pub async fn list_machines(&self, session: &str) -> Result<Vec<AccountMachine>, NetworkError>;
    pub async fn revoke_machine(&self, session: &str, machine_id: &str) -> Result<(), NetworkError>;

    // Editor grant for Relay: session auth, explicit target + ephemeral cnf.
    pub async fn issue_editor_grant(&self, session: &str, target_machine: &str, cnf_x_b64u: &str)
        -> Result<GrantToken, NetworkError>; // audience ast-relay, scopes relay:connect+forge:execute, ttl ≤300

    // Config (Vilde skillsync owns semantics; controller only caches version):
    pub async fn latest_bundle(&self, session: &str) -> Result<BundleHead, NetworkError>; // { version, content_hash } + ETag revalidation
}

/// Opaque session token. Never logged, never displayed; Debug redacted.
pub struct SessionToken(/* private */ String);
```

`AccountMachine { id, label, public_key_jwk_x, created_at, revoked_at, last_seen_at }`,
`AccountProfile { account_id, email, display_name, totp_enrolled, machines, latest_config_version }`
mirror Core `/v1/me` exactly (snake→camel per Core wire names).

Errors: `NetworkError::{Transport, Core{code, message, request_id}, Offline}` —
user text is composed by the controller (plain language, no secret/ID pasting),
never from Core messages verbatim.

## Proposed controller side (owned here, `editor-core/src/account.rs`)

```rust
pub enum AccountPhase { SignedOut, Requesting, Pending{..}, EmailApproved{..}, Finalizing, Authenticated{..}, Error{..} }
pub enum AccountEvent { Changed } // Entity<Account> owned by Core
pub struct RelayConnectRequest { account_id, machine_id, machine_label, forge_ed25519_b64u, grant_jwt, grant_expires_at, cnf_secret: [u8;32] /* zeroized on drop */ }
pub trait RelayBackend: Send + Sync {
    fn open_channel(&self, request: RelayConnectRequest) -> gpui::Task<anyhow::Result<()>>;
}
```

`Core` holds `Option<Arc<dyn RelayBackend>>` (setter for Vilde's transport;
default `None` → UI shows the relay-pending state honestly). Controller
prepares everything up to the channel: inventory from `/v1/me` on login
and on resumed-session validation, ephemeral `cnf`, session-authed grant
issue, then hands `RelayConnectRequest` to the backend. Routing inputs use
`artisan_account::sync::decide_route` with paired-direct liveness; the
Forge↔machine binding and Relay presence feed come from Vilde's side.

Session file: `<editor-data>/account.json`, `0600`, atomic tempfile+rename
(same pattern as `hosts.rs` `write_json` plus unix mode). Contents:
`{ base_url, session_token }` only — profile/machines are re-fetched, never
cached, so revoke takes effect on next launch.

## Open items for root/Vilde

1. Confirm module path `artisan_account::network` + names above (or rename once).
2. Vilde's unified `/v1/grants/issue` client fix (Forge 404) is the same
   endpoint the Editor path uses with session auth — one shared builder.
3. `HostConfig`↔account-machine binding (which paired direct host is which
   enrolled machine) is undefined; native Connect for Relay waits on it.
4. `ae account enroll` prints human text; native pairing parses the code
   line defensively. A `--json` flag would remove the parse (Vilde/ae call).
