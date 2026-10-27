# Artisan MCP

- Status: design (2026-10-17); nothing implemented
- Scope: a first-party MCP server, served through the Forge MCP gateway, that lets an agent use
  the app the way the user does: search and read every thread, write to threads, spawn agents on
  any harness, change settings, manage hosts, projects and extensions, and move the Editor
- Builds on: the MCP gateway in `marketplace-and-engine-sync.md` (commit `7f50719d`; removed from
  the tree by the reform in `a290f316`, decision 2 there: every MCP server goes through the
  gateway)

## Outcome

Anything the user can do in the app, an agent in a thread can do too, through one MCP server that
every harness sees the same way. Three things this unlocks:

1. **Recall.** "Do it like in the thread where we fixed the scene bound" works: the agent
   searches every thread, finds the messages, and reads around them.
2. **Harness agents.** Any agent can spawn a subagent on any harness and model. Claude can hand a
   task to Codex; Codex can spawn a Claude agent, or a Codex agent on a different model and effort,
   which its own `spawn_agent` cannot do (a native Codex subagent inherits its parent's
   configuration).
3. **The app is agentic.** Settings, hosts, projects, MCP servers, skills, the global prompt and
   navigation are tools, declared once and shared with the settings UI and `ae`.

## What exists today

- Forge serves an authenticated HTTP API (`forge-http/src/routes.rs`): threads (create, page,
  update, items, agent items, messages, cancel, settle, unread), item answers, content ranges and
  source indexes, settings by key, MCP server mutation, engines and default models, drafts, blobs,
  skills, prompts, and pairing.
- Threads already have `parent_thread_id` (`artisan-api/src/thread.rs:42`); the sidebar hides
  children (`editor-sidebar/src/list.rs:128`).
- Native subagents are `ItemKind::Agent` items (`artisan-api/src/item.rs:74`) with the engine's
  own `agent_id`; the Editor opens one with `conversation.open_agent` and reads it from
  `/threads/{id}/agents/{agent}/items`.
- The inspector's Agents section draws every row with the same bot glyph
  (`editor-inspector/src/lib.rs:327`), from `InspectorAgent { id, label, description, status }`
  (`artisan-api/src/request.rs:310`). The composer already has a logo per engine
  (`editor-composer/src/picker/mod.rs:76`).
- Engines receive the user's MCP servers as raw descriptors (`forge-engines/src/customise.rs:117`,
  `acp.rs:181`). There is no gateway yet.
- Full-text search covers thread titles and project names only (`forge-store/src/schema.rs:141`).
- State is split: Forge owns threads, projects, engines, MCP servers, skills, the global prompt and
  Forge settings; the Editor owns hosts (`editor-core/src/hosts.rs`), its device `settings.json`
  (frame rate, FPS overlay, typography, glass) and navigation.

## Decisions

1. **A regular MCP server behind the gateway.** The Artisan server is the gateway's first-party
   installation at `/mcp/artisan`. Engines reach it like any other installation, over HTTP.
   Outside clients can reach it the same way later.
2. **The credential is the caller.** Each thread's gateway address carries its own credential, so
   the Artisan tools know the calling thread. "This thread" and "my agents" need no ids.
3. **Harness agents are threads.** A spawned agent is a child thread (`parent_thread_id`) with its
   own harness, model, effort and permissions, not a native subagent. It shows in the parent like
   a native agent.
4. **One registry for settings.** Each setting is declared once; the settings UI, the MCP and `ae`
   all read the declaration.
5. **Agents never raise their own power.** Permission mode, trust and destructive changes stay with
   the user (see Approvals).

## Gateway, the parts this needs

Built (`forge-http/src/gateway.rs`, `forge-runs/src/gateway.rs`, `forge-mcp`):

- A plain-HTTP listener on `127.0.0.1:0`, bound at startup next to the mTLS API. It serves
  `POST /mcp/artisan/<credential>` as MCP Streamable HTTP with JSON responses (no SSE stream, no
  session): `initialize`, `ping`, `tools/list`, `tools/call`; notifications get `202`. `GET` and
  `DELETE` answer `405`.
- The credential is a random 24-byte token per thread, kept in memory for the Forge's lifetime
  and carried in the URL path, because not every engine can send headers to an MCP server. An
  unknown credential answers `404`. A Forge restart issues new ones; runs launch with the
  current address.
- Injection at launch, only when the gateway is up:
  - Claude: an extra inline `--mcp-config` JSON (the CLI takes several). The per-home config file
    is shared by every thread, so the per-thread address can't live there.
  - Codex: the app-server is shared by threads, so the server goes in each thread's
    `thread/start` config: `mcp_servers.artisan.url` and `tool_timeout_sec = 600`.
  - Grok and Cursor: an `http` entry in ACP `session/new` `mcpServers`.
  - OpenCode: merged into `OPENCODE_CONFIG_CONTENT` as a `remote` server.
- The server name `artisan` is reserved; `mcp_server_add` refuses it.

Still to build: the per-run call log, `search_tools` / `call_tool` for engines that load every
definition into each prompt, keys for outside MCP clients, and proxying third-party servers
behind the same endpoint (secret store, OAuth, stdio supervision).

## Tools

Tool names are verbs on nouns. Every tool that takes `thread` defaults to the caller's thread.
Every list is paged with a cursor and a hard cap; nothing returns a whole thread at once.

### Recall

| Tool | Does |
|---|---|
| `threads_search` | Ranked full-text search over message text, plans, commands, errors and edited paths. Filters: project, engine, date range, archived, parent. Returns thread, item, snippet, score. |
| `threads_grep` | Regex over item text. Returns `thread · item · line` hits with context lines, capped like `rg -m`. |
| `thread_read` | Reads a thread from a cursor, or around an item (`around`, `before`, `after`). Filters by kind (messages, tools, plans) or agent. Large outputs come back as content ranges. |
| `threads_touching` | Threads that edited or read a path, newest first. |
| `thread_list` | Threads by project, state (working, unread, settled, archived) and parent. |

Raw tool output is indexed apart from the conversation and searched only when asked
(`include: ["output"]`), so logs do not drown messages.

### Threads

| Tool | Does |
|---|---|
| `thread_create` | Project, harness, model, effort, permissions, first prompt. |
| `thread_send` | A message as a new turn, queued, or as a steer. |
| `thread_cancel` | Stops a running thread. |
| `thread_update` | Rename, regenerate the title, archive, settle, mark unread. |
| `requests_list` | Items waiting on the user, across threads. Read only (see Approvals). |

### Harness agents

| Tool | Does |
|---|---|
| `harnesses` | Each harness with sign-in state, the complete model catalog as JSON: efforts and notes, speeds, variants, defaults, exact context windows and override values, image support, descriptions, labs and gateways, from the same catalog as the composer. OpenCode reasoning choices use `variant`, not `effort`. |
| `agent_spawn` | Starts a child thread: `task` (the label), `prompt`, `harness`, `model`, `effort`, `speed`, `variant`, `context_window`, `options`, `permissions` (at most the caller's), `background`. Omitted fields inherit from the parent (see Defaults). Returns the agent id at once, with warnings for anything it filled in. |
| `agent_wait` | Waits up to a timeout for one or more agents; returns each one's state and, when done, its final reply. |
| `agent_message` | Sends a follow-up to a running or finished agent. |
| `agent_read` | Reads an agent's thread (same shape as `thread_read`). |
| `agent_cancel` | Stops an agent and every agent it spawned. |

### Settings

| Tool | Does |
|---|---|
| `settings_list` | Every registered setting with key, type, current value, owner, description and risk. |
| `settings_get` / `settings_set` | One setting by key; `set` validates against the declaration. |

### Hosts

| Tool | Does |
|---|---|
| `hosts_list` | Hosts with kind (Local, WSL, Remote), address and connection state. |
| `host_add` | Local or WSL directly; Remote starts pairing and the user enters the code shown on the other machine. |
| `host_remove` / `host_switch` | Remove needs approval; switch moves the Editor. |

### Projects and extensions

| Tool | Does |
|---|---|
| `projects_list` / `project_add` / `project_remove` | Remove needs approval. |
| `mcp_servers_list` / `mcp_server_add` / `mcp_server_remove` | Add and remove need approval. The list shows each HTTP server's sign-in state. |
| `mcp_server_sign_in` | OAuth sign-in for an HTTP server that answers 401; returns the link, Artisan keeps the token for every harness. |
| `skills_list` / `skill_set_enabled` | |
| `global_prompt_get` / `global_prompt_set` | |
| `harness_install` / `harness_set_default_model` | Install needs approval. |

### Navigation

| Tool | Does |
|---|---|
| `open_thread` | Opens a thread, optionally scrolled to an item. |
| `open_settings` | Opens a settings page or section. Tools that change settings, hosts, MCP servers, the global prompt or harnesses open their page only with `navigate_user: true`. |
| `reveal` | Opens the inspector, an agent, or a file reference. |

## Harness agents

**Spawn.** `agent_spawn` creates a child thread under the caller's project with
`parent_thread_id` set, the requested harness and configuration, and the prompt as its first
message. Nothing from the parent's conversation is copied; the prompt is the whole brief. The
parent gets an `ItemKind::Agent` item keyed by the MCP tool call, so the conversation row and the
inspector treat it like a native agent: the row reads "Delegated <task>", a follow-up reads
"Messaged <task>".

**Defaults.** With no configuration, the agent inherits its parent's harness, model, effort, speed
and permissions. With a partial or invalid configuration, each missing or unusable field is filled
in and the result carries a warning naming what was filled in and why, so the caller can respawn
if it meant something else:

| Given | Filled in | Warning |
|---|---|---|
| nothing | everything from the parent | none |
| some fields, all valid for the parent's harness | the rest from the parent | none |
| a different harness, no model | that harness's default model, the parent's effort if the model supports it, else the model's default | "no model given for codex; using gpt-6 (its default)" |
| a model the harness doesn't have | the harness's default model | "codex has no model claude-opus-5-5; using gpt-6" |
| an effort or speed the model doesn't support | the model's default | "gpt-6-luna has no effort max; using medium" |
| permissions above the parent's | the parent's | "permissions capped at auto (the parent's)" |
| a harness that isn't signed in | nothing; the spawn fails | an error saying to sign in |

Model settings on `agent_spawn` and `thread_create` use the ids advertised by `harnesses`.
`context_window` takes a catalog `context_windows[].override_tokens` value, not the displayed
window size. `options` merges over inherited engine options only for the same harness and model;
explicit options can also be supplied when switching. A top-level `context_window` wins over
`options.context_window`; `options.context_window: null` restores the standard window.
Unsupported variants and context overrides fall back to the model's defaults with warnings.

**Codex gets configurable subagents.** Through `agent_spawn`, a Codex agent picks any harness,
model, effort, speed, variant, context window and engine options the user has, instead of inheriting its parent.

**Results.** `agent_spawn` returns at once. A parent that needs the answer calls `agent_wait`,
which blocks up to its timeout (under every harness's MCP call timeout) and can be called again. A
background agent's final reply is delivered to the parent as a queued message when it finishes, so
the parent does not have to poll.

**Permissions.** A child's permission mode is at most its parent's. A child's approval prompts go
to the user in the child thread and surface in the parent's Agents section; a parent can never
answer them.

**Limits.** Depth is capped (default 3). `agent_cancel` and cancelling the parent stop the whole
tree. Each child's usage counts against its own harness and shows on its own thread.

**Item shape.** `ItemKind::Agent` gains `engine: Option<EngineId>` and `thread:
Option<ThreadId>`. Native agents carry the parent's engine and no thread; harness agents carry
both. `open_agent` opens a harness agent's child thread in the same agent view, read from
`/threads/{child}/items` instead of the agent items route.

## Agents panel shows the harness

Each row in the inspector's Agents section draws the agent's harness logo where the bot glyph is
today, using the composer's engine logos moved into `editor-ui` so both share them.
`InspectorAgent` gains `engine` (and `model`, shown muted after the label). Native and harness
agents look the same apart from the logo, so a Claude thread with a Codex agent reads at a glance.
The status dot stays.

## Recall index

- A new FTS5 table over item text, written in the same transaction as the item: user and
  assistant messages, plan steps, command lines, error text, and edited or read paths.
- Tool output goes in a second table, searched only with `include: ["output"]`.
- A path table (`thread`, `item`, `path`, `op`) backs `threads_touching`.
- Existing threads are indexed once by a background backfill at Forge start, newest first.
- Grep runs the regex over FTS candidates when the pattern has a literal, and over a bounded scan
  otherwise; both stop at the cap and return a cursor.

## Settings registry

```rust
pub struct SettingDecl {
    pub key: &'static str,
    pub owner: Owner,          // Forge or Editor
    pub kind: SettingKind,     // Bool, Enum, Number { min, max }, Text
    pub default: Value,
    pub label: &'static str,
    pub description: &'static str,
    pub risk: Risk,            // Plain or NeedsApproval
}
```

The settings pages render from the declarations; the sections that today say "not yet
configurable" (thread titles, usage recovery and others) become real when declared.

## Editor-owned tools

Hosts, device settings and navigation live in the Editor. Forge forwards those calls to the
Editor over its existing connection and waits for the answer; with no Editor connected, they fail
with "the Editor is not open". The Editor answers from the same code paths as its UI.

## Approvals

An action marked `NeedsApproval` shows an approval card in the calling thread, with the same flow
as tool permissions, and runs only once the user approves:

- removing a host or project
- adding or removing an MCP server, installing a harness
- any setting declared `NeedsApproval`

No tool can raise a thread's permission mode, answer another thread's approval, or read secrets.

## Delivery phases

Built in one pass (2026-10-17) rather than phase by phase:

- Gateway, injection for every engine, the recall index with its backfill, and all the tools above.
- Harness agents: child threads with `spawned_by`, the `Agent` item's `engine` and `thread`,
  defaults from the parent with warnings, permissions capped at the parent's, depth 3, tree
  cancel, `agent_wait`, and background results delivered as a user-role message starting
  `[Artisan] Agent …`, tagged `from_agent` so the Editor shows it as the agent's report in the
  work ("Reported back <task>", opening on the agent's reply) rather than as the user's bubble.
  MCP is request and response only: the server can't put anything into an idle model's context,
  so a new turn carrying a user-role message is the only way to wake the parent. Claude Code
  delivers its own background tasks the same way. Agent threads start from their prompt alone; only forks inherit the
  parent's history.
- Agents panel: the harness logo and model per agent; a harness agent opens its own thread.
- Forge-owned approval cards for removals, new MCP servers, the global prompt and installs.
- Editor bridge: `Event::EditorCall` to the connected Editors, answered on
  `POST /editor/calls/{id}`; Forge gives up after 90 s with "No Editor answered".
- Results are short markdown for both the model and the user who opens the call: titles and
  words first, ids in code spans. Opening a call shows only its result. Rows read as verbs
  ("Searched threads for zombie", "Read “Connected MCP servers”"); Forge puts the named thread's or
  agents' titles in the call's `title`. Recall leaves out calls to Artisan's own tools.
- `summary_titles` is a real Forge setting with a switch under Settings → Threads → Assistance.

Verified by `forge-http` test `a_thread_recalls_another_thread_and_runs_a_codex_agent_over_the_gateway`:
a scripted Claude thread finds another thread's marker through `threads_search` over the real
gateway, spawns a Codex agent, and gets its reply from `agent_wait`. The recall index was also run
against a copy of a real 38k-item database (backfill ~9.5 s, search ~7 ms).

Not yet verified against real binaries: Claude with two `--mcp-config` values, and Codex's
per-thread `mcp_servers` override in `thread/start`. The settings pages are not yet rendered from
the registry; the registry drives only the MCP.

## Open questions

1. Scope of recall: every project by default, or the caller's project unless it asks wider?
2. Should a harness agent show in the sidebar under its parent, or only through the parent?
