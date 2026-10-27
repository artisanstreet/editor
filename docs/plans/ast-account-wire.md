# AST account wire: inner application contract v1

Audience: the browser UI contributor (routed via root). This is the exact
contract the browser speaks inside an open Noise channel. Relay sees only
opaque bodies. Source of truth for shapes:
`crates/artisan-api/src/tunnel.rs` (strict requests, forward-compatible
reads). Version: v1, additive fields only.

## 1. Layers (what the browser implements)

```text
Browser ──WSS──> Relay ──WSS──> Forge daemon
  outer WS frames:      Relay transport envelopes (Relay contracts/wire.md):
                        hello / authenticate / open / data / ack / close / pong
  inner Noise channel:  Noise_XX_25519_ChaChaPoly_BLAKE2b (Relay contracts/e2e.md):
                        handshake (3 messages) → confirm (JWT-in-confirm) → open
  inner application:    THIS DOCUMENT (JSON request/response, chunking, event streams)
```

The browser opens the channel (Noise initiator), presents its Core grant
JWT inside the encrypted confirm, and only then sends application calls.
The Forge verifies the grant itself (Core JWKS) and enforces scopes.

## 2. Request / response shapes (exact property names)

```jsonc
// client → Forge, one Noise DATA plaintext (after open)
{
  "id": "caller-made string, echoed back for pairing",
  "method": "GET | POST | PUT | PATCH | DELETE",
  "path": "/state",                      // origin-form only: never a full URL,
                                         // never credentials, never `..`
  "body": { /* optional JSON; omitted for GET/DELETE */ }
}

// Forge → client, one Noise DATA plaintext
{
  "id": "echo of the request id",
  "status": 200,                         // HTTP-style status of the local route
  "body": { /* JSON, or a $bytes envelope */ },
  "content_type": "image/webp"           // optional: set when body is $bytes
}
```

- Unknown request fields are rejected (400). Responses may add fields;
  receivers ignore what they do not understand.
- `path` keeps its query string (`/threads/<id>/items?limit=50`).
  `..` segments and non-`/`-relative paths are rejected (400).

## 3. Binary bodies (the `$bytes` envelope)

Frames stay JSON while carrying bytes:

```jsonc
// PUT /blobs (WebP upload), or GET /blobs/<sha> answers:
{ "body": { "$bytes": { "content_type": "image/webp", "b64u": "<base64url>" } } }
```

- Requests: when `body` is exactly `{"$bytes": {...}}` the Forge sends
  the decoded bytes with that content type; anything else is sent as JSON.
- Responses: non-JSON local answers (images, exported sources) come back
  as a `$bytes` envelope with `content_type` set beside `body`.

## 4. Chunking (> 60 KiB bodies)

Noise plaintext cap is 60_000 bytes per frame. Anything larger — a big
`POST /threads/<id>/messages` with images, a transcript page, a blob —
is chunked at the application layer (Noise order is guaranteed, so parts
arrive in order):

```jsonc
// each chunk frame (JSON, ≤ 60 KiB plaintext):
{ "transfer": "<caller-made id, e.g. the request id>",
  "index": 0,            // zero-based; completes at index == total - 1
  "total": 3,
  "total_bytes": 140000, // receiver verifies the reassembled length first
  "bytes_b64u": "<≤40 KiB of raw bytes, base64url>" }
```

- 40 KiB raw per chunk: base64url expands it to ~54.6 KiB of text, so
  even the largest chunk frame stays under the 60 KiB Noise plaintext cap
  after JSON framing (asserted in `artisan-api` chunk tests). The Forge
  also enforces a 64 MiB per-connection aggregate across in-progress
  transfers (expansion-aware), not only the per-transfer cap.
- 32 MiB of raw bytes per transfer max (matches Forge blob/source ceilings,
  so a large transcript never becomes silently unreadable — oversize is an
  explicit error, not truncation).
- At most 820 parts per transfer (`TRANSFER_MAX_PARTS` = 32 MiB in 40 KiB
  pieces, rounded up). Receivers validate `total` against this cap BEFORE
  allocating the parts table, so a corrupt `total` fails closed instead of
  attempting a huge allocation. A standard 32 MiB / 40 KiB transfer (820
  parts) is fully supported; 40 KiB remains the sender's chunk size and
  sane part sizes are still accepted.
- At most 64 concurrent in-progress transfers per reassembly scope
  (`REASSEMBLY_MAX_IN_FLIGHT`); further first-chunks are rejected until
  one completes. Completed transfers are removed and their budget released
  immediately, so back-to-back large responses (even reusing a transfer
  name) each reassemble cleanly and the 64 MiB aggregate never leaks.
- Chunked request: first reassembled bytes must parse as `TunnelRequest`.
  Chunked response: reassembled bytes parse as `TunnelResponse`.

## 5. Reading threads (what the browser calls)

| need | call |
|---|---|
| connect snapshot | `GET /state` → projects, threads, latest runs, settings, engines, `head` seq, waiting items |
| thread window | `GET /threads/<id>/items?before=&after=&limit=` |
| reader rows + work spans | `GET /threads/<id>/transcript?before=&after=&limit=` |
| work section contents | `GET /threads/<id>/work?after=&before=&limit=` |
| agent sub-transcript | `GET /threads/<id>/agents/<agent>/items?...` |
| inspector summary | `GET /threads/<id>/inspector` |
| send / steer | `POST /threads/<id>/messages` `{id, text, images?, config?, draft?}` |
| stop a run | `POST /threads/<id>/cancel`, carry on `POST /threads/<id>/continue` |
| answer approval/question | `POST /items/<id>/answer` `{type: approval|question, ...}` |
| images | `GET /blobs/<sha256>` (`$bytes` image/webp), upload `PUT /blobs` (`$bytes`) |
| engines / models | `GET /engines`, `PUT /engines/<e>/default-model`, `PUT /engines/<e>/model-config` |
| settings (plain switches) | `PUT /settings/<key>` |
| drafts | `GET|PUT /drafts/<scope>` |
| projects | `POST /projects`, `DELETE /projects/<id>`, `GET /projects/<id>/icon|repository`, `GET /fs/list?path=` |
| skills / prompt | `GET /skills`, `/skills/catalog`, `/prompt`, `POST /skills/<id>/<action>`, `PUT /prompt` |

## 6. Live updates (event streaming over the tunnel)

SSE event streams are tunneled as ordered DATA frames on a dedicated
channel per subscription — not degraded to whole-state polling:

1. Browser sends `{"id", "method": "SUBSCRIBE", "path": "/events?since=N[&summary=true][&thread=<id>]"}`
   on an open channel.
2. Forge answers `{"id", 200, {"$stream": {"stream": "<id>"}}}` and starts
   forwarding: backlog first (same `since` cursor semantics as direct
   `/events`), then live envelopes, each as
   `{"$event": {"stream": "<id>", "envelope": {...}}}`.
   Oversized envelopes travel chunked (§4); a lagged broadcast or a full
   per-stream queue (256 envelopes) ends the stream with a `Reset`
   envelope — the browser re-subscribes from its last seq (standard
   `since` cursor, no new bookkeeping).
3. End of stream: `{"$event": {"stream": "<id>", "end": "<reason>"}}`.
   Closing the Relay channel cancels the stream (fail-closed); the
   channel stays usable for unary calls.
4. Concurrency is preserved: streams run on their own channels with
   per-channel Noise sessions, so unary calls proceed while a stream is
   open. `UNSUBSCRIBE` (`{"method":"UNSUBSCRIBE","path":"/events/<stream>"}`)
   ends one stream without closing the channel.
5. Polling remains as a fallback if the browser has no stream open:
   `GET /state`, compare `head`, re-fetch `transcript?after=` on advance.
   Optimistic send: render the sent message immediately with its
   client-made `id`; reconcile when the run's items arrive (retried sends
   are idempotent by `id`).

## 7. Cafe browser scenario (untrusted shared computer)

- The browser keeps auth material in memory only: the ephemeral Noise
  static and the ephemeral `cnf` key. Nothing secret in `localStorage` /
  disk. Tab close drops the channel keys — but tab close does NOT revoke
  the Core session: Core sets a persistent HttpOnly `ast_session` cookie
  on sign-in (JS never sees the token; the cookie jar holds it), and that
  session remains valid until revoke/expiry. On a shared computer the user
  MUST explicitly sign out (session revoke); closing the tab alone leaves
  the Core session alive.
  (Native Editor contrast: it persists only the opaque session token in
  `{editor-data}/account.json`, mode `0600`, atomic replace — reloaded on
  launch, cleared on sign-out/revoke. Same rule: quitting the app does
  not revoke; sign out does.)
- Grants are short TTL (≤ 300 s); the browser refreshes via the Core
  session and re-handshakes (new channel) rather than persisting.
  A stranded ≤300 s grant is useless without the in-memory `cnf` secret,
  but the Core session cookie is the credential that matters on a shared
  machine — revoke it.
- Provider credentials the user types into vault forms (API keys) DO pass
  through browser memory to Core over TLS — the browser never stores or
  logs them, and they are never written to disk, but do not claim the
  browser "never sees" submitted secrets. What never reaches the browser:
  other machines' assigned releases, engine OAuth tokens, and `claude.ai`
  cookie sessions (Forge-assigned release only, Forge-local execution).
  The browser sees only thread content the user opens — execution happens
  on the Forge.
- Assume shoulder-surfing: no secret values rendered by default; tokens,
  keys, and cookie values are never displayed or logged (redacted `Debug`,
  generic error messages with `requestId` only).

## 8. Auth summary for the browser flow

1. Core magic-link + optional TOTP (Core owns the lifecycle). The browser
   never sees the session token: it lives in the HttpOnly `ast_session`
   cookie. The native Editor instead persists only the opaque session
   token in `{editor-data}/account.json` (`0600`, reloaded on launch,
   cleared on sign-out/revoke).
2. Browser fetches its machine list / target Forge from Core, mints an
   ephemeral `cnf` key, and gets a Core grant JWT (`aud=ast-relay`,
   `sub`=account, `mid`=target Forge, scope `relay:connect`+`forge:execute`).
3. Browser opens the Relay WS, `hello` (grant) → `challenge` → `authenticate`
   (PoP `Ed25519(cnf_sk, "ast-relay-auth-v1" || nonce || jti)`).
4. Browser opens a channel, runs Noise XX as initiator, sends confirm with
   the grant JWT inside (encrypted), verifies the Forge responder confirm
   against the Forge enrolled key from the authenticated Core machine
   record, then sends `TunnelRequest` frames and `ack`s every received
   Relay data frame.
