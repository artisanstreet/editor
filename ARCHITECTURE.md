# Architecture

Artisan is two programs. **Forge** runs coding agents ("engines": Claude Code,
Codex, OpenCode) on a machine and keeps every thread in one
SQLite database. The **Editor** is a GPUI desktop app that shows and drives
the threads of one or more Forges. They speak JSON over HTTPS with mutual
TLS. `ae` is the host binary: installer, service manager, and Forge itself.

`RADICAL_REFORM.md` records why it is shaped this way. This file is the
current shape.

## Crates

```
shared
  artisan-api          wire types: ids, Item, Thread, Run, Event, requests, TLS helpers
  artisan-mentions     the `$skill`/`@connector` mention grammar (composer and Forge)
  artisan-markdown-index  Markdown leaf index Forge stores and the Editor reads

forge
  forge-content        which texts and collections become stored sources       → api
  forge-store          rusqlite schema, single writer thread, blobs, legacy import  → api, content
  forge-engines        Harness contract, adapters, installs, sign-in, usage, skills → api, content
  forge-runs           live runs: send, steer, answer, cancel                       → store, engines
  forge-http           axum routes, mutual-TLS auth, SSE event stream, pairing      → runs, store
  ae (bin)             CLI, installer, `ae forge serve`                             → http, engines, store

editor
  editor-ui            theme, components, text editor, markdown  → editor-assets, editor-audio
  editor-core          Forge client per host, connection, HostData, Screen         → api
  editor-sources       viewers for stored sources (raw text, collections)          → core, editor-ui
  editor-sidebar       projects, thread inbox, history, host switcher               → core, editor-ui
  editor-composer      draft, WebP attachments, model picker, send/steer            → core, sources, editor-ui
  editor-conversation  the open thread: item window, viewport, pure render         → core, sources, editor-ui, composer
  editor-inspector     shells, agents, changed files, context/usage                 → core, sources, editor-ui, conversation
  editor-settings      preferences, engines, skills/MCP/prompt, hosts & devices     → core, editor-ui
  editor (bin)         window, titlebar, layout, palette, notices, first pairing    → all
```

Dependencies point down. The one sideways link is Inspector → Conversation,
read-only.

## Data model

Everything a thread shows is an `Item` (`artisan-api/src/item.rs`):
`{id, thread_id, position, seq, run_id, parent_id, native_id, status, kind}`.
`kind` is one of user message, assistant message, tool (shell, read, edit,
search, web, MCP, view image, other), agent, question, plan, image,
compaction, notice, error. Subagent items carry `parent_id` = their `Agent`
item. Approvals live on the item they gate (`status: AwaitingUser`).

A `Run` is one engine process lifetime for a thread; `Thread` holds the
title, `EngineConfig`, `activity_seq`/`read_seq` (unread), archive flag and
`parent_thread_id` (handoffs).

Portable handoffs summarize a disposable source-session fork with OpenCode's
structured compaction template, then carry role-labelled recent exchanges
(including tool calls and bounded results). OpenCode uses its native compaction
endpoint on a fork; Claude Code and Codex use summary side queries. Native
Claude summarizes from its saved message UUID in the existing conversation,
retaining its model, effort and tool definitions. The summary is a sibling
branch; Forge keeps the original continuation bookmark. Missing sessions and
cross-thread forks use canonical history in a fresh temporary conversation. A
failed summary fails the handoff without delivering the pending message. The summary
is stored on the compaction item for recall. Runs retain their launch config so
later forks can preserve cache-sensitive settings after a harness switch.

The summarizer always inherits the source conversation's model and launch
settings, including effort and permissions, to preserve prompt-cache reuse.
Each summary attempt logs input, cached-input and output token usage. The
harnesses still own their in-session automatic compaction.


Text deltas, reasoning, shell output and progress are **live only**: sent as
`Event::Live` with no seq, never stored. The stored item gets the final text.

Images are WebP blobs addressed by SHA-256, max 32 MiB; the Editor encodes
before upload, Forge converts engine-saved images.

## Sync

One global `seq`. Every stored change (project, thread, item, run, setting)
takes the next seq in the writer thread and is published in commit order.

- `GET /state` → projects, threads, latest runs, settings, engines, items
  awaiting the user, and `head`.
- `GET /events?since=N` (SSE) → every change after `N`, then live. The
  stream subscribes before reading the backlog and dedups by seq, so nothing
  falls between. If the client is too far behind (or lags the broadcast), it
  gets `Reset` and refetches `/state`.
- `GET /threads/:id/items?before|after|limit` → pages of a thread's items.
- `GET /threads/:id/transcript?before|after|limit` → the transcript as the
  reader sees it: `limit` counts rows (messages, replies, prompts, errors,
  images, compactions), and the work between them comes as `WorkSpan`
  summaries (`Item::folds_into_work`, stored as `items.work`). A live run's
  newest work ships as items. A turn of any length is one page.
- `GET /threads/:id/work?after&before&limit` → the work in a span, newest
  first, for a work section that is open.

The Editor's connection (`editor-core/src/connection.rs`) is a plain state
enum: disconnected → connecting → live → backoff → retrying, resuming from
its last seq after any drop. `HostData` holds what that Forge sent;
sections read it and subscribe to `HostEvent`.

## Runs

`forge-runs::Hub` keeps one live run per thread (`thread → inbox, cancel`).

- **Send** stores the user message as `Running`. If the thread has a live run
  the message is a **steer** and goes to its inbox; otherwise a run starts.
- A run takes every undelivered message (`Running` user messages), marks them
  `Done`, and drives the engine's `Harness`. When it ends, undelivered
  messages start the next run. A steer is never lost and there is no queue.
- **Answer** routes an approval/question decision to the run by the item's
  `native_id`. **Cancel** cancels the run; open items settle as `Cancelled`.
- A Forge restart marks live runs failed and resumes undelivered messages.

### Engines

`forge-engines/src/harness.rs` is the contract: `Harness::run(Turn, RunIo)`
gets the input, an inbox (`Steer`, `Answer`) and a cancel token, emits
`EngineEvent`s keyed by adapter-local keys, and returns an `Outcome`.
Each adapter is a pure decoder (lines in, events out) plus a thin driver:

| Engine | Transport |
|---|---|
| Claude Code | `claude -p` stream-json over stdio; titles via `generate_session_title` |
| Codex | `codex app-server` JSON-RPC over stdio |
| OpenCode | `opencode serve` HTTP + global `/api/event` SSE |

An approval rides on the item it gates (`Tool`/`Plan`, `AwaitingUser`), keyed
like that item, so an answer reaches the engine by the item's native id.
Decoders keep their own copy of each item they may still change; once it
settles they keep only its stored-source descriptors (`retained.rs`), which is
what bounds their memory.

Adapters are tested with recorded transcripts
(`forge-engines/transcripts/<engine>/*.jsonl`) and insta snapshots.

Engines install into `<data>/engines/<engine>/versions/<v>` with `current`
and `previous` (an `Install` state enum); each runs with its own `HOME`
under `<data>/engines/<engine>/home`. Skills (`<data>/extensions/skills`),
MCP servers and the global prompt (settings `mcp_servers`, `global_prompt`)
are materialised into each engine's own config format at launch.

## Auth and pairing

Each Editor has a self-signed certificate (`identity.json`). Forge has one
too. Every request is mutual TLS: Forge accepts any client certificate at the
handshake and authorises by looking it up in `devices`; the Editor pins
Forge by the SHA-256 fingerprint of its certificate.

Pairing: `ae forge pair` makes a one-time 8-character code (10 minutes,
stored hashed) and prints `address code fingerprint`. The Editor calls
`POST /pair` with the code and its certificate, over a connection already
pinned to that fingerprint. For a Forge on the same machine or in WSL the
Editor runs `ae forge pair --json` itself.

## Editor

One window. `Core` owns the paired hosts and the current `Screen`
(`Home | Thread | NewThread | Settings(page)`); `artisan://` links parse into
a `Screen` at the edge. Each section is one GPUI entity that owns its state,
reads `Core`/`HostData`, and calls Forge itself through `ForgeClient`.
No central command enum; the binary only lays sections out.

The conversation keeps one copy of the open thread: a window of items keyed
by position, upserted from events; live text from `HostData`; UI state per
item. The **Viewport** (Following ⇄ Reading → Jumping; inputs in, effects
out) decides which range stays loaded and fetches transcript pages at the
edges. Work the window covers but has not loaded stays a `WorkSpan`: a
closed section is drawn from it, an open one shows it as an earlier-work
row that loads `/work` as it nears the view. Body text is prepared by
`Bodies`, which the conversation owns; rows draw a prepared `Body` set on
them and are cached views. Render is a pure function of those. Run phase
and disclosure are computed, not stored. An opened thread stays hidden
until its first screen is ready (its edges read, its text in view
prepared), so nothing in view moves after it shows.

## State machines

Plain `enum State` plus a transition function, only for real lifecycles with
transitions that matter: Forge connection (editor-core), engine Install
(forge-engines), Viewport (conversation), Attachment (composer). No state
machine library. Everything else is data plus derived functions; stale async
work is cancelled by dropping its task, not fenced with generation counters.

## Files

| Where | What |
|---|---|
| Forge `<data>/forge.sqlite3` | the store (WAL) |
| Forge `<data>/blobs/` | WebP images by hash |
| Forge `<data>/engines/` | engine installs and homes |
| Forge `<data>/extensions/skills/` | skills |
| Forge `<data>/forge.lock` | one Forge per data dir |
| Editor `<config>/Artisan/editor/` | `identity.json`, `hosts.json`, `settings.json` |
| Install root `~/.local/share/artisan` or `%LOCALAPPDATA%\Artisan` | `versions/`, `current`, `bin/ae`, `forge/` (data) |

`ae forge import` copies a pre-reform Forge (threads, messages, images,
skills, engine installs and homes) into the new data dir. Old tool calls and
reasoning are not carried over.

## Development

`nix run .#dev` builds `ae` and the Editor with Cargo, runs a dev Forge from
`~/.local/share/artisan-dev` on port 47901, and launches the Editor (in WSL,
the Windows build). `nix run .#dev -- import` imports the old dev Forge.
`nix run .#release` is the same loop built with Cargo's release profile.
See `nix/dev.nix`. Release archives: `nix build .#release`, signed by
`scripts/release`.

## Tests

A test must be able to fail for a reason a user would notice. There are
three kinds:

- engine transcripts → item snapshots;
- Forge scenarios in process (`forge-runs`, `forge-http`, `editor-core`
  against a real Forge on a temp dir);
- section scenarios with `gpui::TestAppContext`.

No tests of derives, constants, bounds or single private structs.
