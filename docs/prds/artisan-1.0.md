# Artisan 1.0 — The Artisan Street Ecosystem

Status: planned — this is the 1.0 release gate. Artisan does not ship 1.0
before every milestone in this document passes its acceptance criteria.

## Summary

Artisan pivots from a standalone local tool to an account-attached ecosystem
product. One identity — the **Artisan Street Account** — owns your
preferences, your engine credentials, and your fleet of machines. Signing into
a brand-new laptop materializes everything: every host you have ever attached,
your settings, your installed harnesses. Adding a machine no longer involves
pairing links, IPs, port forwarding, SSH, or any third-party network product:

```sh
# On any host, no Editor required:
curl -fsSL https://artisan.st/forge | sh
ae attach
# → opens (or prints) a link: "Sign in to your Artisan Street Account
#    to attach DESKTOP-96USC6J to your fleet."
```

Approve, and the host appears live in the Machine dropdown of every Editor
you are signed into, forever.

This requires first-party infrastructure — identity, fleet directory, relay,
and encrypted sync — collectively the **Street services**. Building and
hardening them is in scope for 1.0 and gates the release.

## Product Principles

- **Identity is the spine.** Machines are possessions of the account, not
  per-box configuration. Everything the user owns follows their sign-in.
- **The cloud is blind.** Street services route ciphertext and store sealed
  blobs. User content, preferences, and engine credentials are end-to-end
  encrypted under keys only the user's devices hold. A total compromise of
  Street infrastructure must not disclose user secrets or allow fleet
  impersonation.
- **Every host runs the same autonomous Forge.** Loss of connectivity to
  Street degrades to local-only operation; it never bricks a machine or loses
  data. Forge remains the state authority for its own threads and projects.
- **The Editor is a stateless client.** It renders whatever fleet the account
  resolves to, from any machine, desktop or browser.
- **One code path.** Local machine, WSL distro, remote workstation, and
  (future) burst VM are all rows in the same Machine dropdown, attached by the
  same mechanism, trusted by the same key model.
- **No third-party network dependencies.** No Tailscale, no SSH transport, no
  UPnP/NAT tricks. Outbound WSS to Street is the only connectivity primitive.

## Terminology

| Term | Meaning |
| --- | --- |
| Account | An Artisan Street Account; the root of identity and ownership. |
| Fleet | The set of hosts attached to one account. |
| Host | A machine running Forge and enrolled in a fleet. |
| Owner device | A device holding account keys (an Editor the user signed into). |
| Forge | The autonomous backend on every host (unchanged role). |
| Editor | The stateless client (Electron or browser). |
| Street services | First-party cloud: Identity/Entitlements, Directory, Relay, Sync. |
| Organization | The owner of product licenses; every account gets a personal org. |
| License | An org's entitlement to a product plan, e.g. `{product: "editor", plan: "free"}`. |
| Attach flow | The `ae attach` device-authorization sign-in that enrolls a host. |
| Harness | An installed agent engine (Claude, Codex, future native harness). |
| Sealed blob | An E2E-encrypted payload stored or routed by Street services. |

## Domains

The apex is `artisan.st`. Products live as subdomains; each product's backend
lives under `<product>.api.` so the ecosystem scales past the Editor without
renaming anything.

| Surface | URL |
| --- | --- |
| Landing / product pages | `artisan.st` (Editor page at `artisan.st/editor`) |
| Editor web app (includes attach approval and fleet UI) | `editor.artisan.st` |
| Editor control-plane API (attach, directory, sync) | `editor.api.artisan.st` |
| Editor relay (WSS) | `editor.relay.artisan.st` (regional `<region>.editor.relay.artisan.st` later) |
| Ecosystem identity (sign-in ceremony, tokens, passkeys) | `id.artisan.st` |
| Account and org console (members, licenses, billing) | `account.artisan.st` |
| Install transports | `artisan.st/editor/windows`, `artisan.st/editor/unix`, `artisan.st/forge` |

Identity is deliberately not under `editor.*`: accounts and orgs are
ecosystem-level. Product backends never mint identities — they consume
`id.artisan.st` tokens and check entitlements.

## What Changes From Today

| Today (verified prototype) | 1.0 |
| --- | --- |
| Pairing links / codes as the user-facing trust bootstrap | Removed from the product surface. Sign-in is the only user-facing trust act. |
| "Master Forge" holds canonical prefs/credentials, movable role | Role dissolves into the account. Canonical state is sealed blobs in Sync. |
| Machine added via link-code paste | `ae attach` + browser sign-in + approval. |
| Renderer pairs to local Forge via `ae open --handoff` | Unchanged, but internal-only: it is the machine-local bootstrap, never shown to users as a concept. |
| Loopback-only Forge listeners and gates | Unchanged. Remote reachability is exclusively via the host's outbound relay connection. |
| WSL distros brokered by the local Forge | Unchanged mechanically; discovered distros auto-enroll as hosts under the signed-in account. |

Nothing in `modules/protocol` data-plane, `modules/transport` framing, or the
renderer's conversation runtime changes. The 1.0 work adds a control plane and
identity layer around the verified architecture.

## User Journeys

1. **First run, fresh machine (Editor installed).** Launch → sign-in screen
   (passkey; email OTP fallback) → this machine enrolls as a host
   automatically → the user's existing fleet, preferences, and harness state
   materialize. Time target: under one minute to a familiar environment.
2. **Headless host.** Forge-only install one-liner → `ae attach` → the command
   prints a URL and an 8-character user code (and opens the browser when one
   exists). User signs in on any device, reviews the host's name, platform,
   and key fingerprint, approves. The host appears live in every signed-in
   Editor. No Editor is ever installed on the host.
3. **New laptop.** Install Editor → sign in → fleet appears. The final trust
   step is a one-tap approval on an existing owner device (preferred) or a
   recovery code. The new laptop becomes an owner device.
4. **Run work elsewhere.** In the thread environment panel, the Machine select
   lists the fleet: this machine, its WSL distros, every attached host, each
   with a presence dot. Picking one creates the thread on that host's Forge.
   Nothing else to configure.
5. **Harness replication.** Installing an engine in Settings offers "install
   on all hosts". Credential sync is per-host opt-in, visible at attach time
   and editable later. Credentials travel as sealed blobs between devices;
   Street never holds plaintext.
6. **Revocation.** From any Editor or `account.artisan.st`: remove a host → its
   device key is dropped fleet-wide, its relay session is terminated, its
   sealed-blob access ends. Removing an owner device additionally rotates
   wrapped fleet keys.
7. **Recovery.** Passkey lost → recovery code restores account access and
   re-establishes owner keys. Losing all owner devices and all recovery codes
   loses the sealed data — by design; the consequences are stated plainly in
   onboarding.

## The Attach Flow (specification)

Shape: OAuth 2.0 Device Authorization Grant (RFC 8628), carrying a device key
enrollment.

1. `ae attach` generates (or loads) the host's long-term keypair and calls
   `POST attach/start` with the public key, host metadata (hostname, platform,
   arch, forge version), and a proof-of-possession signature.
2. Response: `{verification_url, user_code, device_code, interval, expires_in}`.
   TTL 15 minutes, single use. The CLI prints the URL + code and opens the
   browser if the host has one.
3. The user signs in at the verification URL (`editor.artisan.st/attach`,
   any device; sign-in redirects through `id.artisan.st`). The approval page
   shows the pending host: name, platform, key fingerprint, requesting IP,
   and the credential-sync opt-in checkbox.
4. On approval, an owner-key-holding context (the approving browser session
   via passkey-PRF, or an owner device relaying through Sync) wraps the fleet
   keys to the host's public key and posts the sealed enrollment.
5. The host, polling `attach/poll`, receives: host record, relay credentials
   (token bound to its key), and the sealed enrollment it alone can open. It
   connects outbound to Relay and appears in the fleet.
6. Failure cases: expired code (restart), denied (explicit terminal state),
   rate limits per IP and per account, replayed device_code rejection. Every
   attach attempt writes an audit event.

`ae detach` (host-side) and Console/Editor removal (owner-side) are the
inverse; both end in the same revocation path.

## Identity and Security Model

- **Authentication:** passkeys (WebAuthn, resident keys) as primary; email
  OTP as fallback and for account creation. Sessions are short-lived tokens
  refreshed by key possession. No passwords at 1.0.
- **Key hierarchy:** an Account Root Key wrapped by (a) passkey PRF outputs
  per owner device and (b) recovery codes → Fleet Key → per-blob content keys.
  Hosts hold their own device keypairs; owner approval wraps the Fleet Key to
  enrolled host keys. All wrapping is client-side.
- **Sealed blobs:** preferences, settings subset, harness credential bundles,
  and fleet metadata labels are E2E-encrypted. The Directory stores only what
  routing operationally requires in plaintext: account id, host public keys,
  presence, protocol versions.
- **Peers never re-propagate secrets.** Only owner-device contexts initiate
  credential replication. A compromised host yields that host's copy, never
  push access to the fleet.
- **Threat model (summary):** compromised Relay → traffic analysis only;
  compromised Directory/Sync → ciphertext and metadata, no impersonation
  (approvals are signed by owner keys); stolen host → revocable, holds only
  its own wrapped material; stolen signed-in laptop → OS-level session risk,
  mitigated by passkey re-auth for sensitive operations and remote sign-out;
  Street insider → same as compromise, blind by construction.
- **Auditability:** attach, approve, revoke, recovery, and sync-policy events
  are recorded per account and visible in the Console.

## Street Services (infrastructure to build)

The Street backend is written in **Gleam on the BEAM** (Wisp + Mist for
HTTP/WSS, `gleam_otp` actors, Postgres via typed bindings, OTP releases).
Two constraints force this intersection and only Gleam satisfies both:

1. **The runtime must be the BEAM.** The relay and attach flows are huge
   numbers of long-lived, stateful, isolated connections with polling storms
   and presence fan-out — per-process heaps, preemptive scheduling, and
   supervision trees are the product's survivability story.
2. **The language must be statically typed.** Street is built agent-first,
   and the compiler is the agent's tightest verification loop; a sound type
   system with exhaustive matching and fast compiles is the primary QA
   instrument, not a comfort.

Haskell was seriously considered (excellent types, capable green-thread
runtime) and rejected on operational grounds: one failure domain, global GC
tails on long-lived connections, space-leak pathology, slow compiles in the
agent loop. Elixir was rejected as the application language for lacking
static types, but remains the substrate for **typed FFI**: security-critical
ceremonies use battle-tested Erlang/Elixir libraries (WebAuthn via Wax,
supervisors where `gleam_otp` is young) behind narrow, audited, typed Gleam
boundaries. The FFI surface is enumerated and reviewed; application logic is
pure Gleam.

Two deployables plus Postgres and object storage:

1. **`street-id`** — identity + entitlements (`id.artisan.st`,
   `account.artisan.st`). Product-agnostic by construction.
2. **`street-editor`** — the Editor control plane + relay
   (`editor.api.artisan.st`, `editor.relay.artisan.st`). One OTP release;
   the relay can split out under real load without an API change.

Contracts between the TypeScript clients and the Elixir services are a single
generated boundary: schemas defined once (Effect Schema in a shared contracts
package, as the repo already does for its own protocol), OpenAPI emitted from
them, and the Gleam side verified against the spec with contract tests in
CI. No hand-maintained duplicate types.

### 1. Identity and Entitlements (ecosystem)

Accounts belong to people; **organizations own licenses**. Every account gets
a personal org at creation; teams come later on the same shape. A license is
a typed entitlement — `{product: "editor", plan: "free"}` — checked by
product backends on every session. Editor is the first consumer, never a
special case: `street-id` contains no Editor tables, and adding a future
product is a new license id, not a schema change.

Scope: account creation, passkey registration/assertion, email OTP fallback,
sessions and token issuance (OIDC-shaped so future products and third-party
integrations are standard), org membership, license records, owner-device
registry, recovery codes, audit. Storage: Postgres.

### 2. Directory

Fleet state: hosts, public keys, platform metadata, presence (fed by Relay),
entitlement flags (future billing hook). The authorization reference for every
relay connection and sync access.

### 3. Relay

The blind router, unchanged in concept from the registry design: terminates
outbound WSS from hosts and Editors, routes E2E frames by fleet/connection id,
emits presence. Stateless per connection; horizontally scalable;
region-expandable later. Artisan's existing multiplexed binary transport
tunnels through unchanged inside the E2E layer.

### 4. Sync

Versioned sealed-blob store with a change feed (delivered over Relay). Blobs:
preferences, settings subset, harness credential bundles, fleet labels.
Conflict policy: last-writer-wins per blob at 1.0 with version vectors
reserved for later. Storage: Postgres + object storage for large payloads.

### 5. Web surfaces

Two, with distinct owners:

- **`account.artisan.st`** (part of `street-id`): ecosystem console — orgs,
  members, licenses, sessions, owner devices, recovery codes, audit, data
  export/delete. Billing lives here later.
- **Fleet UI inside `editor.artisan.st`**: the Editor web app itself hosts
  attach approvals (`editor.artisan.st/attach`, the URL `ae attach` prints),
  host management, and sync policy. No separate console product; the Editor
  is the console for its own fleet.

### Cross-cutting

- **Gateway:** one edge (TLS, rate limiting, request logging) in front of the
  monoservice.
- **Observability:** structured logs, metrics, alerting on attach-flow error
  rates, relay connection churn, sync lag. A public status page.
- **Environments:** dev/staging/prod with seeded test fleets; staging is used
  by the product repo's E2E gate.
- **Data:** automated backups, tested restore, account export and deletion
  (sealed blobs are exported as-is; deletion is real).
- **Abuse:** per-IP and per-account rate limits on attach and OTP; disposable
  fleet quotas.
- **Repo:** sibling repository in the GitHub org (`artisanstreet/street`),
  two Gleam applications sharing internal packages; the shared contracts
  package is published from this repository (or a small
  `artisanstreet/contracts` repo) and consumed by both sides.

## Client-Side Work

### Editor

- Sign-in surface (first run and Settings): passkey ceremony, OTP fallback,
  session state, owner-device approval sheet ("MacBook wants to join").
- Machine dropdown fed by the fleet (Directory via home Forge), presence dots,
  separator, "+ Attach a machine…" opens instructions (the `ae attach`
  one-liner) instead of any pairing UI.
- Renderer multi-connection runtime (from the fleet phase plan): connections
  per host, thread→host binding, aggregated thread list.

### `ae` / Forge

- `ae attach`, `ae detach`, `ae fleet` (list/status), forge-only install mode
  in the landing transport.
- Fleet client in Forge: outbound relay connection, presence, sealed-blob
  sync client, harness replication executor.
- Host broker (WSL): discovered distros provision and auto-enroll under the
  signed-in account; no separate attach for WSL.
- Local bootstrap (`ae open --handoff`, loopback gates) unchanged, internal.

### Protocol

- New control-plane surface: `fleet.query`, `fleet.events`, host presence,
  attach-status streams, sync-policy commands. Kept strictly separate from
  the data plane, per the settled design, so future roles (team members with
  use-but-not-manage rights) become an authorization check, not a refactor.

## Migration and Compatibility

- The verified WSL prototype path (wrapper + env overrides) remains a dev
  tool; the product path is fleet enrollment.
- **Local-only escape hatch:** first run pushes sign-in as the path; a
  deliberately quiet "use this machine only" mode keeps Forge fully
  functional offline (threads, projects, local engines). It exists because
  the autonomy principle demands it and because enterprise/self-host needs
  it; it is not a marketed mode. Open question 1 finalizes its visibility.
- **Self-hosted Street** for enterprises (the opsec fallback): explicitly
  designed-for (monoservice + Postgres is self-hostable by construction) but
  out of scope for 1.0 delivery. Nothing in 1.0 may preclude it — no
  hardcoded domains, endpoint set is fleet configuration.

## Out of Scope for 1.0

Burst VMs and billing (the entitlement hooks exist, the features do not);
teams/multi-user fleets; self-hosted Street delivery; NAT hole-punching
(relay-only is acceptable); SSH anything; Tailscale anything; mobile clients.

## Milestones and Acceptance Criteria

### M1 — Street foundation

`street-id` + account console: account creation, personal orgs, licenses,
passkeys, OTP fallback, sessions, recovery codes, owner devices. Accepted
when: a fresh browser can
create an account, register a passkey, sign out, recover via code; audit
events recorded; rate limits enforced; staging deployed with backups.

### M2 — Fleet: Directory, Relay, attach

Host enrollment end to end. Accepted when: `ae attach` on a clean headless
Linux VM and a Windows machine both enroll via browser approval in under two
minutes; hosts show live presence; revocation kills the relay session in
under ten seconds; E2E frames between two hosts traverse the relay with the
relay demonstrably unable to decrypt (test vectors in CI); attach abuse
limits verified.

### M3 — Sync and replication

Sealed-blob sync + harness replication. Accepted when: preferences set on one
machine appear on another signed-in machine; a Claude harness install fans
out to an opted-in host with working credentials; a non-opted-in host
provably receives no credential material; blob storage passes the
export/delete test.

### M4 — Editor integration

Sign-in in the product, fleet-fed Machine select, multi-connection runtime,
WSL auto-enroll. Accepted when: the full journey — sign in on a fresh
Windows machine, see an existing remote host and a WSL distro in the
dropdown, create a thread on each, run a Claude turn on each — passes as an
automated E2E against staging (CDP harness exists from the WSL verification
work).

### M5 — Hardening

Security review of the key model and attach flow; load test relay (target:
10k concurrent host connections on one node before splitting); chaos pass
(Street outage → local mode degrades correctly, reconnect recovers);
recovery-flow usability pass; documentation.

### M6 — 1.0 ship gate

All of: M1–M5 accepted; the distribution PRD's Windows gate plus forge-only
Linux install path released; new-laptop journey under one minute on video;
status page live; on-call/alerting defined; account deletion verified. Then,
and only then, 1.0 ships.

## Open Questions

1. Visibility of the local-only mode in first-run UX (recommendation: one
   quiet link, never a wall).
2. Email provider for OTP and the sender identity (`no-reply@artisan.st`).
3. Relay regionalization order after single-region launch.
4. Whether WSL distros appear as distinct fleet hosts or nested under their
   Windows host in the dropdown (recommendation: nested visual, distinct
   host records).
5. Exact contract-generation pipeline (Effect Schema → OpenAPI → Gleam
   contract tests) and where the generated artifacts live.
6. Whether `street-id` should expose full OIDC for third parties at 1.0 or
   only first-party token issuance (recommendation: first-party only,
   OIDC-shaped internally so opening it later is configuration).
