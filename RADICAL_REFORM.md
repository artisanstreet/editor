# Radical reform

> Historical record of the 2026-10-11 audit and the decisions behind the
> first reform pass. It is not kept in sync with the code: `statig` and some
> other choices below were later dropped. `ARCHITECTURE.md` is the current shape.

Findings and decisions from the codebase audit of 2026-10-11. The current
codebase (~440k hand-written production lines, ~209k test lines, ~100k
generated Cap'n Proto) does a job that fits in roughly a quarter of that. This
document records what is wrong, what was decided, and the target shape for the
rewrite. Where a decision was revised during the discussion, only the final
decision is recorded, with the rejected option noted.

Numbers come from `wc`/`grep` and read-only inspection of the live database;
"after" figures are estimates.

---

## 1. Diagnosis

The code is not sloppy line by line. It is careful machinery for problems the
app does not have. Forge is one process holding an OS lock on its own SQLite
file, yet it is built like a distributed system with competing workers. A
vocabulary (lease, fence, custody, receipt, generation, linear, fail-closed)
took hold early and every later work packet copied it to look consistent. An
800-line file-size ratchet, clippy pedantic at deny, and the stacked
work-packet process scattered the code into many small "policy" files and
essay-length comments.

Vocabulary counts in `modules/`: lease 1,970 · receipt 2,337 · fence 815 ·
custody 564.

### 1.1 Backend runtime (~45k non-test lines)

- **One token's path:** ~30 function hops, ~9 representations
  (provider event → `TextDelta` → parts body → coalescer → `IncrementalText` →
  `AssistantChange` → `CommitRunBatch` → SQLite patch row → replay batch →
  protocol frame), 6 async boundaries. Every token round-trips through SQLite
  before reaching the screen.
- **Durable outbox + claim + lease** for message dispatch: 4 worker loops poll
  every 100 ms through write transactions, claim with an owner token, renew
  the lease every third of its lifetime, sweep expired leases. The process
  lock already rules out a second worker.
- **Typestate claim chain:** `LoadedClaim → LaunchedClaim → PreparedClaim →
  BoundClaim`, each re-carrying the same fields.
- **7-field fence on every streamed batch** (`COMMIT_RUN_SQL` checks 16
  columns to bump `updated_at_ms`).
- **Delivery:** a data-less watch wake triggers ~12 SQLite re-reads per
  subscription per batch (~240 queries/s while one reply streams).
- **Two mirrored registries** (`run_cancellation.rs`, `run_interaction.rs`)
  keyed by the same `(ThreadId, RunId)` — one doc comment says it "mirrors the
  other on purpose".
- **God structs:** `RequestHandler` with ~17 optional fields and ~33 `with_*`
  builders; turn state with ~26 fields.
- **Error types:** 77 in the runtime; 241 `enum *Error` repo-wide; 2,624
  `.map_err(`.
- **Dead code:** 20 backend `*_policy.rs` modules (~7.1k lines) referenced only
  by `lib.rs` and their own tests (verified). Repo-wide ~44 such modules,
  ~14k lines, ported from the old TypeScript app.

### 1.2 Engine adapters (~50k production lines incl. `native_engine`)

- No working abstraction. `EngineSocket` has 5 impls; only Codex's `open`
  works; nothing calls `probe()`.
- ~390 per-engine branch sites across 45 files, over 7 parallel
  engine-identity types (`EngineId`, two `EngineSelection`s, `ManagedEngine`,
  `EngineArg`, `ResolvedLaunch`, `InternalLaunch`) plus string constants.
- Copy-paste per engine: executor prologue ×4, continuation gate ×4 (each with
  its own `*Decision` enum), pump loops with 16–18 arguments, two process
  custody stacks, ≥5 "run a CLI with caps" helpers, near-identical
  `grok/probe.rs` and `cursor/probe.rs`.
- **Cursor never runs:** `resolve_cursor_launch` returns `None`
  (`native_run_dispatch/claim.rs:1513`); ~5k lines including tests.
- **Grok discards assistant text** and never answers permission requests.
- Hand-written tar parser (`archive_header.rs`), trust-on-first-download
  records, N kept generations with use-leases.
- **Codex titles:** if `thread.name` is empty, Forge starts a separate Codex
  session with its own title prompt (`operation/codex.rs:405-430`). Every
  harness already generates a title.

### 1.3 Data layer (~41k lines: database 26.7k, domain 11k, migrations 3.4k)

- **39 tables**, 77 indexes, 9 triggers. Four patterns stacked: event ledger,
  projection, patch log, lease + receipt fencing.
- **The database has its own JSON codec** (~45 `Stored*` mirror structs)
  because domain types have no serde; one commit path decodes the backend's
  JSON then re-encodes it observation by observation.
- SHA-256 digest per batch whose only reader is `run_batch_receipts`
  (175k rows, 100% `committed=1`).
- Live DB (265 MB): `conversation_patches` 180k rows / 71 MB,
  `observation_ledger` 47k rows / 73 MB, `composer_draft_save_receipts`
  29.5k rows for 123 drafts. ~431k of ~455k rows and ~162 MB of ~233 MB table
  data are log or ceremony; 62 MB is image bytes stored inline.
- What the 277 MB file actually holds: ~2.5 MB conversation text
  (`conversation_items`); ~50 MB shell output stored as 41k JSON-wrapped
  chunks (`observation_ledger`, `terminal_activity`); 22.5 MB of full-body
  assistant snapshots in the patch log (avg 26 snapshots per message, one
  message has 5,605); 35 MB message images (9 duplicates); 24 MB composer
  uploads, **all 22 orphaned** (zero draft references); ~45 MB of indexes,
  mostly serving receipts, patches and the ledger.
- `terminal/complete.rs`, `fail.rs`, `auxiliary.rs` (~3.5k lines) are one
  function suite with a suffix swapped. `next_revision` exists 6 times.
- SeaORM + sea-orm-migration + sqlx, but in practice raw SQL: 17 entities vs
  104 `from_sql_and_values`, 69 `query_one_raw`, 80 `try_get_by`.
- 8 pooled connections fighting over SQLite's single write lock, plus an
  8-attempt write-retry schedule to paper over the contention the pool
  creates.
- Assistant text is zeroized on drop.

### 1.4 Wire protocol and transport (~38k hand-written lines + ~100k generated)

- Cap'n Proto schema (3.9k lines) re-declares the domain types; ~13.6k lines of
  hand-written encode/decode (387 functions) with hand-typed error paths like
  `"request.queueMessage.attachments.mimeType"`; ~13.6k lines of round-trip
  tests. System capnp drifts; a nix-pinned compiler is required. Zero-copy is
  never used — everything is copied into owned types.
- Transport rebuilds what QUIC streams already give: a correlation registry,
  request lifecycle, the request id repeated 3–4 times and cross-checked,
  `validate_response_family` (~280 lines), version-offer negotiation with
  only v1, a hand-rolled `CancellationToken` imported by ~40 backend files.
- Dead: `event_sequence.rs` (verified), `phase1_proof` schema, ~8 request
  types no client sends. `conversation_activity.rs` is UI logic in protocol.
- Every connection-level thing is stateful: handshake, credential rotation,
  per-connection subscription registry with leases and generations,
  ack-gated cursors, request lanes, connection holds. Desyncs have been seen
  first-hand.

### 1.5 Auth (~9k lines)

One decision at the bottom — **single-use reconnect secrets, rotated on every
connect** — created the rest:

1. Crash between rotation and save locks you out → hand-designed binary record
   (`b"ARTNRC01"` magic, 9 fixed fields), a lock file, a
   `ReconnectSessionLease`, record quarantine.
2. Secrets on disk → ~900 lines of directory/symlink/reparse-point/identity
   checks.
3. On Windows → a 1,555-line ACL validator that shells out to `whoami.exe` and
   `icacls.exe` and reads their (likely localised) text output.
4. Validator diagnostics → canary tests asserting errors don't contain
   `"credential-bytes-canary"`.
5. Around it: zeroize everywhere, constant-time compares, a bootstrap secret
   that converts into rotating ones, invitation files with pid/incarnation
   "reconnect fences", re-importing invitations over `\\wsl$` on every
   connect, `auto:PORT` route detection.

None of it adds security: TLS with pinning stops sniffing; anyone who can read
a user-only file is already the user.

### 1.6 Frontend (~176k lines, 394 files, 182 modules)

- `NativeApplication`: **144 fields**, extended by ~40 `impl_*.rs` files; a
  1,706-line event router; 39 command variants, 70 event variants (mostly
  `X`/`XFailed` pairs).
- Navigates by **URL strings ported from SvelteKit** (`/t/[workspace]/[thread]`,
  `/e/[workspace]/[thread]`) with a route gate policy.
- ~14k lines of transport ceremony (`response_validation`,
  `request_construction`, `mutation_lane`, `connection_holds`,
  `subscriptions`, `request_delivery`, retry diagnostics).
- **Conversation is a projection of a projection:** wire → snapshot/patches →
  `ConversationProjection` → statig delivery machine → per-turn statig
  machines → scene facts (fed separately from `EngineObservationState`) →
  scene → rendered blocks. Four state machines (delivery, turn, disclosure,
  viewport), three on statig, ~9k lines in 10 files. They model protocol
  desync rather than the conversation, and each layer keeps its own copy.
- 69 `pub mod`s with no non-test user (~21k lines).
- 133 `[[test]]` targets, each linking a separate GPUI binary.

### 1.7 UI, assets and the rest

- `ui` (58.7k): ~9.6k lines of dead components (accordion, alert_dialog,
  avatar, command, context_menu, input_group, link_preview, lip_card, sheet,
  slider); three selects, three typeaheads, three hover surfaces;
  hand-written 3D format parsers + CPU rasterizer (4.2k), CSS colour parser,
  terminal parser ("a subset of `vte`"); 13 `*Semantics` structs nobody reads.
- **Evaluation-only font in production:** the wordmark renders in "Test
  Founders Grotesk" (`impl_surfaces.rs:232`); its licence grants **no
  redistribution or production rights**. Legal issue.
- Unused fonts (Artisan Neo, Cal Sans), duplicate variable+static font files,
  a 2.4k-line asset provenance manifest with a 1.2k-line test.
- `catalog`: a hand-written JSON codec over `serde_json::Value`.
- Dates parsed by hand ~9 times; semver and percent-encoding by hand though the
  crates are in the lockfile; HTML metadata parsed by hand.
- Logging: 97 `eprintln!`, no `tracing`; window failures log nothing unless
  the flight recorder is compiled in.
- `tests/policy/dependencies.rs:74` bans `anyhow`, spelling it
  `["any","how"].concat()` so grep can't find it.
- 594 lint suppressions; `#![forbid(unsafe_code)]` repeated in 230 files
  despite the workspace lint; a 102-entry allowlist for the 800-line rule.
- Docs: work packets, plans, interface specs, wave map, review notes, a
  50k-line function map, 1.7 GB `evidence/`.

### 1.8 Limits that kill runs or the window

~490 limit constants. Most are harmless; these turn "big" into "corrupt":

| Limit | Effect |
|---|---|
| `MESSAGE_BODY_MAX_BYTES = 65_536`, `MAX_ASSISTANT_TEXT_PARTS = 128` | run cancelled mid-reply (`turn.rs:1469`) — hit in long agent sessions |
| Claude stream-json line > 1 MiB | run killed (one screenshot or big tool result) |
| Scene 512 items / turns | window `set_failure` (only partly fixed) |
| Command queue 64 (`COMMAND_CAPACITY`) | window failure on a busy Forge (`impl_host_mounting.rs:105`) |
| `MAX_PENDING_EFFECTS = 4096` | window failure; fed by the pump's `_` arm |
| 128 changed files per card | scene build fails → fatal |
| 256 threads per project listing | listing request fails |
| Codex tool output > 8 KiB | output silently dropped |
| Composer > 64 KiB | paste silently truncated |
| Requests per connection | Forge closes the connection to force a reconnect |
| History paging at 384 items | older history becomes unreachable, silently |

33 `set_failure` sites, ~13 conditions; every capacity refusal is reported as
an "integrity" failure.

### 1.9 Polling and performance

- Editor UI polls a `std` channel every **16 ms** (`selectors.rs:71`); the
  transport thread does a blocking `send` into a 64-slot queue, so a UI stall
  freezes networking.
- Forge dispatch polls every 100 ms through write transactions.
- Every streamed batch rewrites the whole body (quadratic bytes per reply);
  WAL checkpoint fsync every second.
- Per delta, the editor rebuilds the whole scene (copying every body); the
  reveal animation parses the full markdown **twice per frame**; each reveal
  step re-parses the growing prefix (quadratic).
- QUIC itself is push-driven. The polling is on either side of it: the
  transport-thread→UI bridge, and Forge's outbox/re-read design.

### 1.10 Sliding window (as built)

Good: opens on the newest 8 turns, pages 8 older turns on scroll, settled
turns' work rows stay on Forge until expanded, off-screen turns render as
placeholders with remembered heights.

Broken: loaded history only grows (nothing evicted), then hits a ceiling
(`HISTORY_MAX_LOADED_ITEMS` = 384) and silently stops — older history becomes
unreachable and jumps are cancelled. Pages count turns, not items. The
renderer is ~1k lines of hand-written windowing over a scroll area instead of
GPUI's variable-height `List`.

### 1.11 State inventory

| | Today | Real truth | Target |
|---|---|---|---|
| `NativeApplication` fields | ~144 (+~90 in sub-structs) | 7 | ~12 in `editor-core`; UI-only state moves into widgets |
| Forge in-memory state | ~52 pieces | 9 | ~6 |
| SQLite tables | 39 | 13 | 7 |
| Distinct files on disk | ~40 | — | ~9 |
| Lock/receipt files | 9 | 0 | 1 |
| Conversation state machines | 4 | 0 | 0 |

Worst duplication:

- **Selected thread in ~10 places** (`selected_thread`, `pending_thread`,
  `route_history.current`, `thread_screen_key`, subscription `active_thread`,
  run controls, composer queue, …). Fits the "frozen thread" incidents.
- **An assistant message's text in up to 10 client copies:** projection,
  transient scene, surface scene, cloned turn per row, reveal prefix, markdown
  cache, scene facts, observation store, questionnaire panel, pending rows.
- **The same text up to 5 times in the DB:** observation ledger, patch log,
  items, `messages`, receipt bodies (~145 MB).
- Approval status in 4 copies; model choice in ~8; draft text in 3 plus sync
  state; thread list 4×; project list 5×; host identity 5×.
- 7 ordering counters where one global seq would do; run state over 5 tables;
  engine run config stored 5 times; engine install truth in 5 files; global
  prompt in 5 places; images stored 3 ways.
- Forge keeps a per-connection copy of everything already sent and diffs it
  against the DB each wake.
- Three overlapping views of "what an engine can use" (engine inventory,
  extension sync, composer catalog) plus model discovery.

Bugs found along the way:

- Settings screen appearance/notification/telemetry fields start from
  constants and are never saved.
- The Forge listener serves one authenticated client at a time.
- Each managed engine has its own HOME with its own `.cargo`, `.rustup`,
  `.npm` (5.7 GB Claude, 6.5 GB Codex).
- 8 leaked readiness temp files; two orphaned state directories.

---

## 2. Decisions

### 2.1 Product features that stay (simplify inside, never delete)

- **Managed engine installs.** Artisan manages skills, MCPs and customisation
  on every engine, so Forge owns which binary/version runs. *Rejected: use
  whatever is on PATH.*
- **The `ae` installer** (`irm … | iex` bootstrap, interactive install of the
  Editor into Windows and a Forge into WSL/Windows/…).
- **The `ae` CLI**, scoped by noun.
- **A network transport** so adding a remote host is free. *Rejected: stdio
  over ssh/wsl.exe.*
- **Usage readers** (`claude -p /usage`, Cursor's endpoint) — the only way
  today; best-effort, failure shows "usage unavailable".
- Steering, approvals/questions, resume vs handoff, attachments,
  remote Forge, reconnect.

### 2.2 Principles

1. **No distributed ceremony.** No leases, fences, custody, receipts,
   generations or claim/renew loops for single-process state. Plain
   ownership, task cancellation, counters. A coordination primitive needs a
   concrete second actor (the engine install lock qualifies: `ae` and Forge
   can both install).
2. **No pre-validation.** Call `std::fs` or SQLite and propagate the error with
   context. Checking first is a race anyway.
3. **One fact, one owner.** Everyone else derives.
4. **Forge owns all data.** The client holds a window plus UI-only state.
5. **UI state is keyed by item id and lives in its widget.**
6. **The protocol keeps no state.** The client's only sync state is `since`;
   the only ids it sends are ones it creates.
7. **Caches are labelled, have a TTL, and are never truth.**
8. **Event-driven, never polled.** Await channels / `Notify`; a full queue
   makes the sender wait, never fail.
9. **Limits only at trust boundaries** (socket frames, engine output lines,
   attachments, fetched content). Overflow truncates that one unit with a
   visible marker; never kills a run or the window. No count limits on
   conversation content.
10. **Rendering is a pure function** `view = f(items)`. Real lifecycles are
    `statig` machines that hold no data copies (§2.15); everything else is
    derived.
11. **Crates are the boundary.** If today's code doesn't fit the unit, remodel
    it.

### 2.3 Crate layout

Split by section, not by layer. The compiler enforces the borders (no cycles,
private by default), dead `pub(crate)` code warns, crates compile in parallel,
and one test binary per crate replaces ~400 test targets.

```
shared
  artisan-api           serde types: Thread, Item, Run, Event, requests

editor
  editor-ui             components, markdown, theme
  editor-core           Forge client per host, event stream, Selection, Screen
  editor-sidebar        → core, ui
  editor-composer       → core, ui
  editor-conversation   → core, ui, composer
  editor-inspector      → core, ui, conversation (read-only)
  editor-settings       → core, ui
  editor (bin)          frame, layout, event forwarding → all

forge
  forge-store           rusqlite schema + queries          → api
  forge-engines         Harness trait, adapters, installs, usage → api
  forge-runs            live runs, run_turn, broadcast     → store, engines
  forge-http            axum routes, auth, event stream    → runs, store
  ae (bin)              CLI, installer, `ae forge serve`   → http, engines
```

Per crate: `lib.rs` exports one entity, its constructor and `apply(&Event)`;
everything else `pub(crate)`; tests inline. Don't split below one crate per
section; keep `artisan-api` and `editor-core` APIs small.

Deleted crates: protocol, transport, domain (folded into `artisan-api`),
database, migrations, catalog, build_info, tracing (→ `tracing` crate),
installer and cli (→ `ae`), native_dev, capnp_codegen, packaging (→ a release
script), audio (pending decision).

### 2.4 Editor sections

```
App            window frame, layout, Selection { host, thread }
├── Sidebar         projects, thread inbox, history, host switcher, new thread
├── Conversation    open thread: item window, transcript, live run
│   └── Composer    draft, attachments, model picker, send / steer
├── Inspector       terminals, agents, changed files, context + usage (reads Conversation)
├── Settings        preferences, engines, skills & MCPs, hosts & devices
└── Overlays        command palette, notices, dialogs
```

- Each section is one GPUI entity owning its state; only it mutates it.
- Dependencies point down; the one sideways link is Inspector → Conversation
  (read-only).
- The only cross-section state is `Selection`/`Screen` in `editor-core`.
- Sections call Forge themselves; no central command enum.
- App only forwards events to their owner (~30 lines).
- Navigation is an enum, not URL strings:

```rust
enum Screen {
    Thread { host: HostId, thread: ThreadId },
    NewThread { host: HostId, project: ProjectId },
    Settings(SettingsPage),
}
```

`artisan://` deep links are parsed once into a `Screen` at the edge.

### 2.5 Protocol: stateless HTTPS + SSE

*Revised from WebSocket; QUIC remains acceptable but not chosen.* Plain HTTP/2
over mutual TLS (axum on Forge, reqwest + a small SSE reader in the editor):

```
GET  /threads                         list
POST /threads                         create
POST /threads/{id}/messages           send (client-generated id → retries are free)
POST /runs/{id}/cancel | /steer
POST /interactions/{id}/answer        approvals, questions
GET  /threads/{id}/items?before=<seq>&limit=N  |  ?after=<seq>
GET  /events?since=N                  SSE: all changes + live text deltas
PUT  /blobs   GET /blobs/{sha256}     images, attachments (raw bytes)
GET  /engines  POST /engines/{e}/install …
```

- **One global seq** in Forge for every change (threads, items, runs,
  settings). The client keeps one number; on reconnect it reopens
  `/events?since=N`. No handshake, no subscription registry, nothing per
  connection beyond the open stream.
- Text deltas are ephemeral, not seq'd; catch-up includes the live item's
  current text.
- **JSON** with serde on the domain types (`#[serde(try_from)]` on bounded
  newtypes). `serde_json` is already in the tree for the engines; JSON matches
  the DB's JSON columns and is debuggable. *Rejected: CBOR, postcard, Cap'n
  Proto.*
- Exact build/version match at connect; mismatch → "update Forge".
- Why not WebSocket/QUIC: HTTP/2 multiplexes streams (no head-of-line
  blocking), works through proxies/tunnels, reaches WSL2 via TCP localhost
  forwarding (QUIC's UDP likely does not in NAT mode), is curl-able, and
  usable by a future web client. HTTP/3 can be added later.

### 2.6 Auth: mutual public keys

- Forge and each Editor mint their own key pair on first start.
- **Pairing swaps public keys.** A short one-time code (10 min) only confirms
  the swap; nothing secret crosses the wire, so future Artisan pairing
  infrastructure can relay it freely and cannot impersonate either side.
- Connections use **mutual TLS**; each side checks the other's certificate
  against its saved list (~100 lines with rustls).
- Revocation: delete the row from `authorized_keys`;
  `ae forge devices [revoke <id>]`.
- Local and WSL pair with zero clicks (the Editor runs `ae forge pair --json`
  itself, locally or via `wsl.exe`); a remote host means pasting one code once.
- No tokens, no rotation, no secret files, no zeroize, no ACL validation.
- Browser client certificates can wait until there is a web client.

### 2.7 Data

**Forge (`forge-store`, rusqlite — 7 tables, one writer thread + read connections, WAL,
`synchronous=NORMAL`, `PRAGMA user_version` migrations):**

```sql
threads (id, project, title, engine_config JSON, resume_token, read_seq,
         created, updated, archived)
items   (thread_id, seq, id, run_id, kind, status, body JSON)
runs    (id, thread_id, status, error, usage JSON, engine_session, started, ended)
drafts  (scope PRIMARY KEY, body JSON)              -- incl. model choice, image hashes
kv      (key PRIMARY KEY, value JSON)               -- prefs, favourites, global prompt,
                                                    -- projects, extensions, telemetry, Forge key pair
authorized_keys (id, name, public_key, created, last_seen)
pair_codes      (code_sha256, expires_at)
```

- An item is anything in a thread: user message, assistant message, reasoning,
  tool call (with its approval), subagent, question, plan, image,
  compaction, notice, error. See §2.10.
- Streaming text stays in memory and is persisted at item boundaries (plus an
  optional debounce). A crash loses at most the partial item; the run is
  marked interrupted.
- Startup recovery is one statement:
  `UPDATE runs SET status='interrupted' WHERE status='running'`.
- Retries: client UUID + `INSERT … ON CONFLICT DO NOTHING`.
- Old databases: best-effort one-time migration of threads, user messages
  and final assistant text (alpha product, one user); whatever does not map
  cleanly is dropped.

**What gets stored.** Forge stores what the user sees in the thread, in its
final state, once. The engine already keeps its full session transcript (that
is what resume uses), so Forge does not mirror it.

| Item | Stored |
|---|---|
| User message | text + image hashes |
| Assistant message | final text, one row overwritten in place |
| Tool call | name, short input summary, status, output head + tail (~16 KB) for display |
| Approval, question | the request and its answer |
| Plan, goal, compaction | latest version |
| Run | status, error, usage, engine session id |
| Images, attachments | files in `blobs/`, referenced by SHA-256 |

**Memory only (live, then discarded)** — the UI shows only the current one and
drops it when it stops being relevant:

- **Reasoning / thinking:** held in `LiveRun`, streamed as events, gone when
  the run ends.
- **Shell output:** held in `LiveRun` per active job, streamed as events, gone
  when the job ends. A reconnecting client gets the current buffer.

Not stored: reasoning, shell output, patches, revision history, raw engine
observations, receipts, digests, checkpoints, ordinals, per-save draft
history. Raw engine output for debugging is an opt-in rotating trace file, not
a table. Today's data would fit in roughly 40 MB of files plus a few MB of
SQLite instead of 277 MB.

**Images live on disk, not in SQLite** (they were raw BLOBs, not base64, but a
folder is still better):

```
<forge data>/blobs/ab/cdef0123….png      # sha256, split by first two hex chars
```

- Write: hash the bytes, write to a temp file, `rename` into place. Same bytes
  → same path, so duplicates are free.
- Serve: `GET /blobs/{sha256}` streams the file straight from disk.
- Clean up: at startup, delete files no item or draft references.
- **Every image is WebP, for every engine, no exceptions.** The client
  converts JPEG, PNG, GIF, BMP and WebP to WebP before `PUT /blobs` (animated
  GIF → animated WebP via libwebp). Smaller uploads to remote Forges; Forge
  never decodes images. Forge only checks the `RIFF…WEBP` header and the size
  limit, stores the bytes, and passes the same file to whichever engine the
  message goes to. No originals, no per-engine formats, no PNG fallback.
  AVIF is dropped everywhere.
  - Lossy, quality **85**, `sharp_yuv` on (keeps coloured text crisp).
    Measured on 32 of our real PNGs: 26.1 MB → 3.7 MB (14%); visually
    identical side by side, including text screenshots. Quality 92 (today's
    setting) gives 19%; lossless 52%.
  - Resize so the long edge fits the largest engine cap; engines downscale
    anyway, so extra pixels are never seen by a model.
  - Keep whichever is smaller of lossy and lossless WebP (flat UI images and
    tiny icons are sometimes smaller lossless).
  - Already-lossy WebP that needs no resize is uploaded as is (re-encoding
    only adds loss).
  - Encoding lives in `editor-composer` with the `webp` crate (libwebp,
    already a backend dependency) using the advanced config with
    `use_sharp_yuv`; Forge's `attachment_policy.rs` and the per-engine
    re-encode ladder go away.
- Why: keeps the database small (faster backups and vacuum, no 35 MB of pages
  dragged through the WAL), files are inspectable, and streaming needs no
  copy into memory.

**Forge files:** `forge.sqlite3`, `blobs/`, `engines/<e>/<version>/` + `current` + one
previous, one process lock.

**Editor files:** `settings.json` (window prefs, reopen host, sidebar
collapsed), `hosts.json` ({name, address, Forge public key}), its key pair.

### 2.8 Minimal runtime state

**Forge (`forge-runs`):**

```rust
struct Hub { live_runs: Mutex<HashMap<ThreadId, LiveRun>>, events: broadcast::Sender<Event> }
struct LiveRun { cancel: CancellationToken, inbox: mpsc::Sender<Steer | Answer>,
                 child: Child, text: Buffer, reasoning: String,
                 shells: HashMap<JobId, String>,   // memory only, dropped when done
                 adapter: AdapterState }
```

Plus TTL caches: model catalog, usage, rich links, project subtitles/icons.

```rust
async fn send_message(hub: &Hub, thread: ThreadId, msg: NewMessage) -> Result<()> {
    let item = db.insert_item(thread, Item::user(msg))?;   // durable first
    hub.publish(Event::ItemUpserted(item));
    if hub.is_idle(thread) { hub.start_run(thread) }      // otherwise deliver as a steer
    Ok(())
}
```

**Editor:**

| Crate | Holds |
|---|---|
| `editor-core` | `ForgeClient` per host, `since: u64`, one `Store` (projects, threads, runs, catalog, installs, extensions, prefs, usage) updated only by events, `Screen`, back stack |
| `editor-conversation` | the open thread's item window + UI state keyed by item id (below) |
| `editor-composer` | focused draft text buffer, undo/IME/selection, attachment previews, model picker UI, last send error |
| `editor-inspector` | terminal screen-parser caches, dialog UI; everything else derived |
| `editor-sidebar` | UI only; lists read from the store |
| `editor-settings` | unsaved form fields only |

```rust
struct ThreadData {
    items: BTreeMap<Seq, Item>,
    live_text: HashMap<ItemId, String>,
    run: Option<Run>,                       // status, usage, context, title, goal
    window: Window { oldest_loaded: Option<Seq>, exhausted: bool, in_flight: bool },
}

fn view(data: &ThreadData) -> Vec<Turn>     // pure: same items → same screen

struct TranscriptView {                     // keyed by ItemId
    scroll: { offset, follow: bool, anchor: Option<ItemId> },
    heights: HashMap<ItemId, f32>,
    disclosure: HashMap<ItemId, bool>,      // user overrides only
    reveal: HashMap<ItemId, usize>,         // visible length, not a text copy
    answers: HashMap<ItemId, AnswerDraft>,
}
```

Applying an event inserts or replaces by id, so duplicates and reordering
cannot corrupt state; if in doubt, refetch `?since=0`.

### 2.9 Sliding window

- Fetch by items, not turns: `?before=<seq>&limit=100` / `?after=<seq>`.
- Keep a few hundred items around the viewport; the live tail always stays
  loaded; evict far items but keep their measured heights.
- Render with GPUI `List` / `ListState` (variable heights, scroll anchoring) —
  we own the fork.
- No ceiling; memory stays flat; every point in history stays reachable.

### 2.10 Engines and the item model

The internal model describes what agents do; each adapter hides its engine's
wiring behind it. This section is the specification for `artisan-api`'s item
types and for `forge-engines`.

#### Items

```rust
struct Item {
    id: ItemId,                  // ours
    thread_id: ThreadId,
    seq: Seq,                    // global; bumps on every change to the item
    run_id: Option<RunId>,
    parent_id: Option<ItemId>,   // set ⇒ this item belongs to a subagent;
                                 // points at that subagent's Agent item
    native_id: Option<String>,   // engine's id (tool_use_id, Codex item id,
                                 // ACP toolCallId, OpenCode part id) — upserts only
    status: Status,              // Running | Done | Failed | Declined | Cancelled | AwaitingUser
    created_at: Timestamp,
    updated_at: Timestamp,
    kind: ItemKind,
}

enum ItemKind {
    UserMessage      { text: String, images: Vec<BlobHash>, steer: bool },
    AssistantMessage { text: String, phase: Phase },          // Commentary | Final
    Tool             { tool: ToolKind, title: Option<String>,
                       output: Option<Output>,                // head + tail + truncated_bytes
                       exit_code: Option<i32>,                // often unknown (Claude)
                       approval: Option<Approval> },          // approvals hang off their tool
    Agent            { name: Option<String>, description: String,
                       background: bool, result: Option<String> },
    Question         { blocking: bool, questions: Vec<Question>,
                       answers: Option<Vec<Option<Vec<String>>>> },   // None = skipped
    Plan             { steps: Vec<Step>,                      // text + Pending|InProgress|Done|Cancelled
                       proposal: Option<String>,              // markdown plan awaiting approval
                       approval: Option<Approval> },
    Image            { blob: BlobHash, caption: Option<String> },     // generated images
    Compaction       { tokens_before: Option<u64>, tokens_after: Option<u64> },
    Notice           { kind: NoticeKind, text: String },      // Goal{state} | Safety | Reroute
    Error            { kind: ErrorKind, message: String, retryable: bool },  // Api|Refusal|MaxTurns|Execution
}

enum ToolKind {
    Shell     { command: String, cwd: Option<String>, background: bool },
    Read      { path: String, range: Option<(u64, u64)> },
    Edit      { changes: Vec<FileChange> },   // path + Add|Modify|Delete|Move{from} + diff
    Search    { pattern: String, path: Option<String>, mode: Grep | Glob | List },
    WebFetch  { url: String },
    WebSearch { query: String },
    Mcp       { server: String, tool: String },
    ViewImage { path: String },
    Other     { name: String },
}

struct Approval {
    request: Command { command, cwd } | FileChange | Action,
    reason: Option<String>,
    options: Vec<ApprovalOption>,     // what the engine offers
    decision: Option<AllowOnce | AllowAlways | DenyOnce | DenyAlways | Cancelled>,
}
```

Rules:

- **Approvals live on the item they gate** (a tool call or a plan). All three
  engine families tie approvals to a tool call; the item goes to
  `AwaitingUser` until answered. No standalone approval items.
- **Subagents:** the spawn becomes an `Agent` item; every item the subagent
  produces gets `parent_id` = that Agent item (nearest ancestor for nested
  agents). `parent_id` both marks the parent and declares "this is subagent
  output".
- **One item per text part**, not per turn. `AssistantMessage.phase` lets the
  view fold commentary once the final answer arrives.
- **Plans and todo lists** are one `Plan` item per run, replaced in place.
- **Live-only events** (memory, never stored): `TextDelta { item_id, text }`,
  `Reasoning { run_id, text }`, `ShellOutput { item_id, chunk }`,
  `ToolProgress { item_id, text }`, `Retry { attempt }`.

```rust
struct Run {
    id: RunId, thread_id: ThreadId, engine: EngineId, model: String,
    status: Running | Done | Failed | Cancelled | Interrupted | Refused,
    usage: Usage { input, output, reasoning, cache_read,
                   cache_write: Option<u64>, context_used, context_window,
                   cost_usd: Option<f64> },
    session_id: Option<String>,      // engine's session/thread id, used for resume
    started_at, ended_at, error: Option<String>,
}
```

"Waiting on the user" is derived (any `AwaitingUser` item in the run), not
stored.

#### Mapping: Claude Code (stream-json over stdio)

| Native | Becomes |
|---|---|
| `system/init` | `Run.session_id` confirmed |
| `stream_event` `message_start` | new `AssistantMessage` (Running) |
| `content_block_delta` `text_delta` / `thinking_delta` | `TextDelta` / `Reasoning` (live) |
| `message_delta` `stop_reason: end_turn` | `phase = Final`; `refusal` → `Error { Refusal }` |
| `assistant` message | finalises text; each `tool_use` block → `Tool` (Running), `native_id = tool_use.id` |
| `user` `tool_result` | `Tool` output + Done/Failed (`is_error`) |
| frames with `parent_tool_use_id` | same decoding, `parent_id` = the Task/Agent `Tool`'s Agent item |
| `system/task_*` | settles background Bash / background Agent items |
| `system/status: compacting`, `compact_boundary` | `Compaction` Running → Done (pre/post tokens) |
| `control_request can_use_tool` | `approval` on the `Tool` (match on `tool_use_id`), `AwaitingUser` |
| `can_use_tool` for `AskUserQuestion` | `Question { blocking: true }`, answered via `updatedInput.answers` |
| `result` | `Run` status + usage; error subtypes → `Error` |
| transcript `ai-title` (after the run) | thread title |

Tools: `Bash`/`PowerShell` → `Shell`; `Read` → `Read`; `Edit`/`MultiEdit`/
`Write`/`NotebookEdit` → `Edit`; `Grep`/`Glob` → `Search`; `WebFetch`,
`WebSearch`; `mcp__<server>__<tool>` → `Mcp` (split on `__`); `Task`/`Agent` →
`Agent` item; `TodoWrite` → `Plan.steps`; `ExitPlanMode` → `Plan.proposal`
with approval; `AskUserQuestion` → `Question`.

#### Mapping: Codex (app-server JSON-RPC)

| Native | Becomes |
|---|---|
| `thread/start` / `thread/resume` reply `thread.id` | `Run.session_id` |
| `turn/started`, `turn/completed` (`completed`/`failed`/`interrupted`) | `Run.status` |
| `item/started`, `item/completed` | create / settle the item, `native_id = item.id` |
| `item/agentMessage/delta` | `TextDelta` (live) |
| `item/reasoning/summaryTextDelta` | `Reasoning` (live) |
| `item/commandExecution/outputDelta` | `ShellOutput` (live) |
| `item/mcpToolCall/progress` | `ToolProgress` (live) |
| `turn/plan/updated` | `Plan` (one per run, replaced) |
| `thread/tokenUsage/updated` | `Run.usage` (`last.totalTokens` = context used) |
| `thread/name/updated` | thread title |
| `item/commandExecution/requestApproval`, `item/fileChange/requestApproval` | `approval` on the item named by `itemId`, `AwaitingUser` |
| `item/tool/requestUserInput` | `Question { blocking: true }` |
| `model/rerouted`, safety buffering, policy errors | `Notice` / `Error` |
| `thread/goal/updated` | `Notice { Goal }` |
| child thread (`thread/started` with `parentThreadId`, or `collabAgentToolCall spawnAgent`) | `Agent` item (`native_id` = child thread id, `description` = spawn prompt); every frame with that `threadId` gets `parent_id` |

Item types: `agentMessage` → `AssistantMessage` (phase from `phase`);
`commandExecution` → `Shell` (`declined` → `Declined`); `fileChange` → `Edit`
(multi-file); `mcpToolCall` → `Mcp`; `webSearch` → `WebSearch`; `imageView` →
`ViewImage`; `imageGeneration` → `Image`; `contextCompaction` → `Compaction`;
`plan` (plan-mode prose) → `AssistantMessage`; `reasoning` → live only.

#### Mapping: OpenCode 2 (HTTP + per-session SSE log)

| Native | Becomes |
|---|---|
| `session.text.delta` / `.ended` | `TextDelta` / `AssistantMessage` Done (ended text is authoritative) |
| `session.reasoning.delta` | `Reasoning` (live) |
| `session.tool.*` (`started`/`called`/`success`/`failed`) | `Tool` Running → Done/Failed — **payload shape needs a capture** |
| `session.step.ended` / `.failed` | `Run.usage` (summed per step, includes cost) |
| `session.execution.*` | `Run.status` (`interrupted` + `reason: user` → Cancelled) |
| `session.compaction.*` | `Compaction` |
| `session.error`, `session.execution.error` | `Error` |
| `session.updated` | thread title |
| `permission.asked` (global `/api/event` stream) | `approval` on the tool — **never wired today** |
| `form.created` | `Question` |
| child session (`parentID`, task tool) | `Agent` item; child session's items get `parent_id` — **needs following child logs** |

Tools: `bash` → `Shell`; `read` → `Read`; `edit`/`patch`/`write` → `Edit`;
`grep`/`glob`/`list` → `Search`; `webfetch`, `websearch`; MCP tools → `Mcp`;
`todowrite` → `Plan`; `task` → `Agent`.

#### Mapping: ACP (Grok, Cursor)

| Native `session/update` | Becomes |
|---|---|
| `agent_message_chunk` | `TextDelta` → `AssistantMessage` per `messageId` |
| `agent_thought_chunk` | `Reasoning` (live) |
| `tool_call` / `tool_call_update` | `Tool` (`kind`: execute → Shell, read → Read, edit/delete/move → Edit, search → Search, fetch → WebFetch, else Other) |
| `plan` | `Plan` (replaced) |
| `session_info_update` | thread title |
| `_x.ai goal_updated` | `Notice { Goal }` |
| `session/request_permission` | `approval` on the tool call, options from the request |
| `elicitation/create` (Grok), `cursor/ask_question` | `Question` (Grok non-blocking, Cursor blocking) |
| `cursor/create_plan` | `Plan.proposal` with approval |
| prompt result `stopReason`, `usage` | `Run.status`, `Run.usage` |

ACP has no subagent concept. `user_message_chunk` is only replayed on
`session/load` and is ignored live.

#### Steering (no queue)

Sending while a run is active is a steer; there is no message queue. The
`UserMessage` (with `steer: true`) is stored first, then delivered:

| Engine | Steer | Cancel |
|---|---|---|
| Claude | write a `user` line to stdin; the CLI folds it in at its next step. If stdin is already closed (run finished), start a new `--resume` run with it | `control_request interrupt` (verify), else close stdin |
| Codex | `turn/steer { expectedTurnId }`; if rejected because the turn ended, `turn/start` a new turn with it | `turn/interrupt` |
| OpenCode | `POST …/prompt` with the right `delivery` value (**probe**) | `POST /api/session/:id/interrupt` |
| ACP | no in-place steer: `session/cancel`, then `session/prompt` with the message | `session/cancel` |

The `UserMessage` stays Running until the engine accepts it, then Done. A
steer is never lost: if in-place delivery fails, it starts the next turn.

#### Titles

Claude (`ai-title`), OpenCode (`session.updated`) and ACP
(`session_info_update`) name threads themselves. **Codex does not**: today's
"Codex title" is Artisan's own side request written back with
`thread/name/set`. Replace it with the first line of the first user message
(truncated) until Codex sends `thread/name/updated`. No model calls.

#### Engine table, installs, usage

- One `EngineId` enum + a data table (binary, args, archive layout, feed URL,
  login, version minimum); one process-custody module; one JSONL transport;
  one ACP client; one generic continuation gate; one turn runner.
- Claude, Codex, OpenCode ~1.2–1.5k lines each; Grok/Cursor ~250–400 on ACP.
- Installs: `tar`/`zip` crates, hash from feed or vendor signature, active +
  one previous, one install file lock. ~2–2.5k lines (from ~6.3k).
- Usage readers best-effort; compaction thresholds move into the catalog.

#### Bugs the mapping work found in today's adapters

- Claude: approvals never read `tool_use_id`, so they can't be linked to their
  tool; subagent child text is probably duplicated (`protocol.rs:544`); child
  tool calls and thinking are dropped.
- Codex: every child-thread frame except lifecycle is dropped, and the spawn
  prompt is discarded — Agent items are empty; child approvals surface
  unlinked; a steer that races `turn/completed` is lost.
- OpenCode: tool, reasoning and compaction events are only flagged, never
  shown; permissions are never answered (only `approval = never` works);
  child sessions raise `SessionMismatch`.
- Grok: drops all assistant text, thoughts and tool calls; never answers
  permissions; uses `session/requestPermission` where ACP specifies
  `session/request_permission` (verify).

#### To probe before implementing

- Claude: `control_request interrupt` exists; `--settings` / `--mcp-config`
  layering keeps the user's own settings.
- Codex: `acceptForSession` for AllowAlways; MCP servers via `-c` or
  thread-start config.
- OpenCode 2: tool and permission payload shapes; what each `delivery` value
  does mid-run.
- Grok: the permission method name.

### 2.10a Skills, MCP servers and the global prompt

Today Artisan manages skills and the global prompt only (not MCP, commands or
settings), via a 30-second keeper loop that rescans every engine home, a
digest ledger for drift, an import/overwrite flow, a content-addressed
read-only store and backups (~7k lines across crates).

**Decision: Forge is the source of truth, and it materialises config right
before every spawn.**

- **Source of truth:**
  - `kv`: global prompt, MCP server list, engine settings.
  - `extensions/skills/<name>/`: plain editable folders.
- **`materialize(engine, home)`** runs before each engine process starts.
  Every run spawns a fresh process, so a change applies on the next turn with
  no restart and nothing to poll:
  - writes the global prompt file (overwrites; no drift detection);
  - places a link for each skill (symlink; copy on Windows without Developer
    Mode); removes links Forge placed that no longer exist; never touches real
    folders the user put there;
  - passes MCP servers and settings through launch channels, never by editing
    files engines rewrite themselves (`.claude.json`, Codex/Grok
    `config.toml`).
- **One-time import** of the skill folders already inside engine homes (17
  each in Claude and Codex) into `extensions/skills/`.

| Engine | Files Forge writes | Passed at launch |
|---|---|---|
| Claude | `.claude/CLAUDE.md`, links in `.claude/skills/` | `--mcp-config`, `--settings` (generated JSON) |
| Codex | `.codex/AGENTS.md` (delete `AGENTS.override.md`), links in `.codex/skills/` | `-c mcp_servers.<name>…` or thread-start config |
| Grok | `.grok/AGENTS.md`, links in `.grok/skills/` | `mcpServers` in ACP `session/new` / `session/load` |
| OpenCode | `config/opencode/AGENTS.md`, links in `config/opencode/skills/` | `mcp` in `OPENCODE_CONFIG_CONTENT` |
| Cursor | — (once it runs) | `mcpServers` via ACP |

Deleted: keeper loop, `global_prompt_engine_files` digests, prompt candidates
and resolution UI, content-addressed store, stage/probe/backups directories,
`engine-inventory.json` cache. `engine_inventory` stays only as the read-only
report of what each engine loaded (for the composer).

### 2.11 Concurrency and performance

- Forge: tokio multi-thread, no blocking calls on workers; one SQLite writer
  thread fed by a channel; persist at item boundaries; deltas fanned out from
  memory via `broadcast`; dispatch is a direct spawn (no poll).
- Editor: one tokio runtime for networking; events into GPUI via an async
  channel awaited by a `cx.spawn` task (no timer); per-item cached views
  (`cached(style)`); markdown parsed incrementally per item revision (re-parse
  only the open tail block); reveal ranges computed once per revision;
  `SharedString`/`Arc<str>` bodies; request frames only while a fade runs.

### 2.12 `ae` CLI and installer

One binary for CLI, installer and Forge; hosts only need `ae`. Two shipped
binaries: `artisan-editor` (GUI) and `ae`.

```
ae install [editor|forge] [--wsl <distro> | --windows]
ae update | ae uninstall | ae doctor [--fix]

ae forge start | stop | restart | status | logs [-f]   [--on wsl:Ubuntu]
ae forge serve
ae forge pair [--json] | ae forge devices [revoke <id>]

ae engine list | status [<engine>]
ae engine install | update | use <version> | rollback | login  <engine>

ae editor open [<path>]
ae telemetry status | analytics | crash-reports
```

Installer keeps: download, one signature check, extract to `versions/<v>/`,
swap `current`, keep previous for rollback, PATH, shortcuts, `artisan://`
handler, retire old processes, the interactive target picker. Cuts: root lock
+ pending-operation markers, identity-revalidating fs primitives
(`install/authority.rs`), staging leases, the per-root local signing key.
Build info via `env!()` at compile time. ~2.5k lines (from ~9k).

### 2.13 Everything else

- **Errors:** `anyhow` + `.context()` inside both binaries; one wire error
  `{code, message}`. Delete the anyhow-ban test.
- **IDs:** `#[serde(transparent)]` newtypes, no validation macros; drop
  `RequestId`, bounded-text types, counter types.
- **Logging:** `tracing` + file appender (journald on Linux); every window
  failure logs its cause chain.
- **Dependencies:** drop capnp, quinn, rcgen, x509-parser (rustls stays for
  mTLS), SeaORM/sqlx, hyper (reqwest only), one of processkit/command-group,
  zeroize, subtle. Keep `statig` for the machines in §2.15. Use `jiff`/`time`, `semver`, `percent-encoding`,
  `scraper`, `tar`, `zip`, `vte`, a colour crate.
- **Lints:** pedantic → warn; delete the 230 per-file forbids, the 800-line
  ratchet, the dependency policy test.
- **UI:** delete dead components and `*Semantics` structs; one select, one
  typeahead, one hover card.
- **Assets:** remove Founders Grotesk now (legal); remove unused fonts and
  duplicates; delete the provenance manifest and its test.
- **Tests:** see §2.14.
- **Docs/repo:** delete work packets, plans, interface specs, wave map, review
  notes, function map, `evidence/`, `result`, `.failed`; keep `STACK.md`,
  `gpui-fork.md`, runbooks, plus one `ARCHITECTURE.md`. Drop the stacked
  work-packet process.

### 2.14 Testing

**Today.** 6,316 tests in 382 files: ~209k lines under `tests/` plus inline
tests, against ~400k production lines. Almost every file tests one internal
type, through its constructor, against its own constants. By test name:

| What it checks | Tests |
|---|---|
| A bound, limit, or "invalid input is rejected" on an internal type | ~1,300 (20%) |
| Leases, fences, receipts, custody, claims, retry policies | ~530 (8%) |
| Cap'n Proto encode/decode of our own frames | ~320 (5%) |
| Derived traits: `Clone`, `Debug`, `Default`, `Eq` | ~280 (4%) |
| Everything else, mostly one-struct state checks | ~3,900 |

Examples: `prompt_max_rejected` (a `Duration::MAX` config is refused),
`derived_clone_debug_and_equality_behave_as_for_an_owned_value`,
`prompt_success_receipt_is_copy_zero_sized`,
`response_correlation_remains_fenced_for_the_appended_arm`.

They fail in two ways:

- **They miss real bugs.** Grok has 22 tests, and the adapter drops every
  word the assistant says. OpenCode never shows a tool call. Codex subagents
  are empty. No test feeds an engine's real output in and checks what the
  user would see. The only recorded transcripts are 10 Claude files.
- **They pin the ceremony in place.** A test per lease, per bound, per
  receipt means deleting the lease breaks 40 tests. That makes the
  machinery look load-bearing and every refactor look dangerous. Nearly all
  of these tests test code this reform deletes.

**Rule: a test must be able to fail for a reason a user would notice.** Test
through a crate's public API, never a private type. Don't test derives,
constants, config validation, or that a bound is a bound. Every bug fixed
adds a test at the level the user saw it, usually a recorded transcript.

**What replaces them (~150–300 tests, ~15–20k lines):**

1. **Engine transcripts** (`forge-engines`, the biggest share). Real JSONL
   recorded from each engine (Claude, Codex, OpenCode, Grok, Cursor), covering
   text, tools, approvals, questions, subagents, compaction, steer, cancel and
   errors. Feed each through the adapter and snapshot the resulting
   `Vec<Item>` with `insta`. Re-record when an engine updates; the snapshot
   diff shows exactly what changed. This would have caught every adapter bug
   in §2.10.
2. **Forge scenarios** (`forge-http`), in process. Build Forge's router on
   an in-memory store and call it directly (`tower::ServiceExt::oneshot`):
   no sockets, no TLS, no child process. The engine is a transcript replayer
   implementing `Harness`. Script what a client does: send, steer mid-run,
   approve, cancel, drop the event stream and resume with `since=N`, restart
   Forge mid-run. Assert on the items the client ends up with. Milliseconds
   each; 20–40 scenarios.
3. **Store** (`forge-store`). In-memory SQLite: item upserts, seq order, page
   queries, crash recovery, and migration of a copy of the real alpha database.
4. **View** (`editor-conversation` etc.). Rendering is `view = f(items)`, so
   test the fold. Feed item lists, including streaming states, and snapshot
   the view model. A few GPUI tests only for interaction: text selection
   follows Chrome, the window slides and evicts, scroll anchoring.
5. **Pure algorithms** get table or property tests: markdown streaming
   reveal, shell command presentation, text navigation, WebP encoding.

One test binary per crate; no `[[test]]` targets in a top-level `tests/`
directory.

**Why this is fast.** Speed comes from the architecture, not the test style:
adapters are a pure decode step separate from process I/O, Forge is a library
behind a router, and the view is a pure fold. Each test fakes only the outside
world (engine process → transcript, network → direct call, disk → in-memory
SQLite, screen → view model), and every line of our own code runs for real.
Today's suite is slow because of linking, not running: 133 test targets, each
its own GPUI binary.

**Real end to end** (real binaries, mTLS, a real engine CLI, a signed-in
account) is a short smoke script run before a release, not on every change.

### 2.15 State machines

**The test:** a state machine is right when something has a lifecycle that
exists *only there*: events arrive over time, order matters, some transitions
are illegal, and entering or leaving a state has effects. If the state can be
computed from items, it's a function, not a machine. Today's conversation
machines failed because they held copies of the data, so they were caches
that drifted.

**Library: `statig`** (already in the tree, 0.4). It provides:

- hierarchical superstates, e.g. one `Live` superstate handles Cancel for
  every live run state;
- state-local data, e.g. `Backoff { attempt }`, `LoggingIn { url, code }`;
- entry and exit actions;
- an async variant for Forge.

Considered and rejected: `rust-fsm` (a neat transition-table macro, but states
can't carry data) and `sm`/`finny` (unmaintained).

**Rules for every machine:**

- It holds lifecycle data only: ids, counters, a target. Never items, text
  or anything Forge already has.
- Handlers don't do I/O. They push `Effect`s into the context, and the owner
  performs them (spawn, write, HTTP call, notify the view).
- One machine per file in the crate that owns it; there's no machine
  framework of our own on top.
- It's tested through the scenarios in §2.14, not one test per transition.

**The machines (all of them):**

| Crate | Machine | States |
|---|---|---|
| `forge-runs` | **Run** (one per `LiveRun`, async) | `Live { Spawning → Running → Draining }` → Done / Failed / Cancelled / Interrupted / Refused. Steer while Running is delivered in place; while Draining it becomes the next turn. Cancel is handled once in `Live` |
| `forge-engines` | **Engine session** (one per adapter, private) | Claude: Spawned → Initialised → Streaming → Result. Codex: Initialising → ThreadOpening → InTurn ⇄ Idle. ACP: Initialising → SessionOpening → Prompting → Stopped |
| `forge-engines` | **Install** (per engine) | Absent → Downloading → Verifying → Installed(version) / Failed. An update runs Downloading → Verifying while the old version keeps serving |
| `forge-engines` | **Sign-in** (per engine) | SignedOut → LoggingIn { url, code } → SignedIn / Failed |
| `editor-core` | **Forge connection** (per host) | Disconnected → Connecting → Live { seq } → Backoff { attempt } → Connecting |
| `editor-conversation` | **Viewport** (per open thread) | Following (pinned to the live tail) ⇄ Reading { anchor } → Jumping { target, loading } → Reading / Following. It also owns history loading: reaching an edge while Reading issues the page fetch |
| `editor-composer` | **Attachment** (per image) | Encoding → Uploading → Ready / Failed |
| `editor-composer` | **Send** | Draft → Sending → Sent / Failed { retry } |

**Not machines:**

- **Item status** is a stored field with one guard (a terminal status never
  reopens), enforced in `forge-store`'s upsert. One `match`, not a machine.
- **Reveal** is animation progress per item id: words shown and when.
- **Work fold** is a derived default (open while the run is live, folded
  once the final answer arrives) plus `Option<bool>` for the user's toggle.
- **Derived from data, never stored:**
  - thread status (working / unread / waiting on you) and sidebar grouping;
  - the turn's phase (thinking, working, streaming, compacting, waiting for
    background work);
  - Send vs Steer and button enablement;
  - the conversation itself.
- **`Screen`** is a plain enum: any screen can go to any other.
- **Pairing** is a row with an expiry.

#### Conversation reform

Today: wire → snapshot/patches → `ConversationProjection` → delivery machine
→ per-turn machines → scene facts (fed separately from
`EngineObservationState`) → scene → blocks. Four machines and ~9k lines, each
layer with its own copy of the conversation. What each becomes:

| Today | Becomes |
|---|---|
| `conversation_delivery_machine` (AwaitingSnapshot / Ready / Recovering / Closed, generations) | Deleted. Sync is the per-host **Forge connection** machine plus one `seq`; a thread has nothing to recover |
| `conversation_turn_machine` (Pending, WaitingForProvider, Compacting, Thinking, Working, StreamingReply, WaitingForBackground, settled states) | Deleted. `fn turn_phase(run: &Run, items: &[Item], live: &Live) -> Phase`: the same answer computed on demand, so it can't drift |
| `conversation_view_machine` disclosure (AutoOpen / AutoClosed / UserOpen / UserClosed / Retired) | Deleted. Derived default + `Option<bool>` user toggle per run |
| `conversation_view_machine` viewport (Following / Detached / Scrolling / Settling, generations) | Kept as the **Viewport** statig machine, merged with history loading; generations go away because a stale page is just items already in the store |
| `ConversationProjection`, scene, scene facts, `EngineObservationState`, `transcript_window` | Deleted. One store and one fold (below) |

The new shape of `editor-conversation`:

```rust
struct ThreadView {
    items: BTreeMap<Seq, Item>,     // the loaded window, upserted from events
    live: Live,                     // TextDelta / Reasoning / ShellOutput, memory only
    viewport: Viewport,             // the one machine
    ui: HashMap<ItemId, ItemUi>,    // reveal progress, user toggles, measured height
}

fn render(view: &ThreadView) -> impl IntoElement   // pure: items + live + ui → elements
```

Events from the connection upsert `items` or update `live`, then
`cx.notify()`. The viewport decides which seq range to keep loaded. `render`
reads all three and holds nothing. A frozen or desynced thread has nowhere to
come from: there is exactly one copy of the conversation in the Editor.

---

## 3. Size

| Area | Today | After (estimate) |
|---|---|---|
| Backend runtime | ~45k | ~6–9k |
| Engines + `native_engine` | ~50k | ~12–15k |
| Database + domain + migrations | ~41k | ~5–7k |
| Protocol + transport + glue | ~38k (+100k generated) | ~1–1.5k |
| Auth | ~9k | ~300 |
| CLI + installer + packaging + build | ~44k | ~5–6k |
| Frontend | ~176k (incl. in-tree tests) | ~70k |
| UI + assets + catalog + audio + tracing | ~69k | ~31–39k |
| Tests | ~209k | ~15–20k |
| **Production total** | **~440k** | **~110–120k** |

---

## 4. Open questions

1. **Right sidebar contents** — assumed Terminals, Agents, changed files,
   context/usage. Complete?
2. **3D model viewer, audio player, charts** — advertised by the
   `artisan-editor` skill; keep (swap parsers for crates) or cut?
3. **Cursor and Grok** — delete now and re-add on the ACP client, or fix
   during the rewrite?
4. **Wordmark font** — license Founders Grotesk or choose another face?
5. **Engine HOME directories** — share the user's toolchain caches instead of
   per-engine `.cargo`/`.rustup`/`.npm`?
6. **Quick fix before the rewrite** — remove the 64 KiB assistant body cap
   that cancels long runs today?

---

## 5. Plan

1. Write `ARCHITECTURE.md` from §2 as the single specification.
2. Build `artisan-api`, `forge-store`, `forge-runs`, `forge-http` fresh, with
   Claude and Codex end to end.
3. Point the editor at it behind a flag via a new `editor-core`.
4. Add OpenCode and the ACP engines; port the installer into `ae`.
5. Carve the frontend into section crates, replacing the conversation stack
   with the item store + pure view.
6. Best-effort migration of the existing database.
7. Delete backend, database, domain, protocol, migrations, transport, cli,
   installer and the dead code.
