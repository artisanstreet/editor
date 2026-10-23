# The claude.ai transport

- Status: implemented (2026-10-21)
- Scope: a second transport for the Claude engine that speaks claude.ai's own chat backend
  directly, because it is the only place Anthropic's one-line recap of the model's thinking
  exists
- Implements: `crates/forge-engines/src/claudeai/` (session, transport, decoder, tools,
  capabilities, harness) and the `LiveKind::Narration` event
- Runbook: `docs/runbooks/claude-ai-transport.md`

## Outcome

Claude Code returns the model's reasoning as prose, and Forge trims that prose into a clause. The
result is half a sentence: `README` for *"README.md doesn't exist, and those MCP docs tools
aren't relevant…"*.

claude.ai returns something better. The provider writes a one-line recap of what the model is
doing and sends it as its own event — the text claude.ai shows above each tool group:

```
event: content_block_delta
data: {"type":"content_block_delta","index":0,
       "delta":{"type":"thinking_summary_delta",
                "summary":{"summary":"Checking the proper procedure before listing files."}}}
```

The public Messages API never sends that event, so no amount of work against
`api.anthropic.com` will produce it. Speaking claude.ai's endpoint does.

## What was found

### The recap is a claude.ai-only protocol event

`thinking_summary_delta` → `delta.summary.summary`. It is the same carrier claude.ai's own web app
consumes: its step label is `summaries.at(-1) ?? "Thinking…"`.

Checked against `api.anthropic.com/v1/messages` and never seen:

- no `thinking` field at all
- `thinking: {type:"adaptive", display:"updates"}`
- `thinking: {type:"adaptive", display:"summarized"}`
- the same with `anthropic-beta: thinking-display-updates-2026-08-18`
- the same with a client tool declared

Nine live captures of the Claude Code CLI's raw frames (`--include-partial-messages`) contained
only `text_delta`, `input_json_delta`, `thinking_delta` and `signature_delta`.

### `thinking.display: "highlights"` is not a server value

The Claude Code CLI offers `--thinking-display summarized|omitted|highlights`, and the Agent SDK
documents `highlights` as *"the API's one-line thinking titles"*. The server disagrees:

```
400 invalid_request_error
thinking.adaptive.display: Input should be 'summarized', 'omitted', 'updates'
```

`highlights` is client-side only. The CLI sends it, the API rejects it, and the CLI latches the
rejection and falls back to `omitted` — so the turn loses its reasoning entirely. The Agent SDK's
own type is narrower still: `ThinkingAdaptive.display?: 'summarized' | 'omitted'`.

The CLI's own `updates` path (the default since it stopped showing thinking in February 2026)
returns *empty* thinking blocks: `display:"updates"` plus the `thinking-display-updates-2026-08-18`
beta, and nothing else.

### claude.ai takes client-defined tools

This is what makes the transport an agent backend rather than a chat toy:

```json
"tools": [{"name": "Read", "description": "Read a file from the local filesystem.",
           "input_schema": {"type": "object",
                            "properties": {"path": {"type": "string"}},
                            "required": ["path"]}}]
```

→ `200`, and the model calls *our* tool:

```
{"type":"tool_use","id":"toolu_01RhhNcVY6QrnqziWrendJoR","name":"read_file","input":{}}
```

claude.ai's own tool union wants an `input_schema` too — a bad shape is rejected with
`tools.0.FrontendRemoteMcpToolDefinition.input_schema: Field required`.

### The tool result comes back in the prompt

A tool result is delivered in two places, and **the prompt is the one the model reads**:

```json
{"prompt": "<the tool's output>",
 "tool_states": [{"toolUseId": "<id>", "name": "<tool>", "content": [{"type":"text","text":"…"}]}]}
```

- `tool_states` says which call is answered. `tool_results` is not a field at all
  (`tool_results: Extra inputs are not permitted`).
- The prompt must not be blank: `""` and `" "` are refused with `No prompt provided`, `"."` is
  accepted. With a neutral prompt the model does not see the result and calls the tool again; with
  the tool's output as the prompt it quotes the result correctly.

### The stream stays open while claude.ai waits

The first request returns in under two seconds and the tool call arrives at once, but the *stream*
stays open: claude.ai holds the turn for four minutes, waiting for the result, and then gives up
and emits a second, stale tool call. The turn really continues on the request that carries the
result, which answers in seconds.

So the read stops as soon as the tool call is whole. Reading the first stream to the end costs
four minutes per tool round; stopping early costs nothing. A live round trip went from 251 s to
8.8 s.

### Two credentials, and only one of them reaches the recap

| Credential | Reaches | Notes |
|---|---|---|
| OAuth bearer | `api.anthropic.com` ✓, claude.ai ✗ | First-party desktop client `89355bc3-cbfd-4382-905b-976645cad410`, scope `user:inference`, redirect `https://claude.ai/desktop/callback`, token endpoint `https://platform.claude.com/v1/oauth/token`. Cloudflare answers 403 for the chat API. |
| Browser session | claude.ai ✓ | `sessionKey` + `cf_clearance`. The clearance is bound to the user agent. |

So a claude.ai session is a *browser* session. The Claude Desktop app has one because it embeds
claude.ai in a webview and logs in there.

### The request shape

```
POST https://claude.ai/api/organizations/{org}/chat_conversations
    {"name": "...", "is_temporary": true}                     -> 201 {"uuid": ...}

POST https://claude.ai/api/organizations/{org}/chat_conversations/{uuid}/completion
    {"prompt": "...", "model": "claude-opus-5-5",
     "thinking_mode": "auto", "effort": "high",
     "rendering_mode": "messages",
     "attachments": [], "files": [], "sync_sources": [], "tools": []}
```

Two things are not optional, and neither is guessable:

- **`effort` gates the recap.** Without it the turn comes back with no `thinking_summary_delta`
  at all. `auto, no effort` → zero recaps; `auto, effort=high` → recaps.
- **`thinking_mode`** is one of `extended`, `standard`, `auto`, `off`. `auto` is what the web app
  sends; `adaptive` and `enabled` are rejected with
  `thinking_mode: Input should be 'extended', 'standard', 'auto' or 'off'`.

claude.ai keeps the conversation itself, so a turn carries only the new prompt. The stream is a
superset of the Messages API stream: the same event names, plus `thinking_summary_delta` and
`disposition` on `content_block_stop`.

### The account's own endpoints

| Endpoint | What it answers |
|---|---|
| `GET /api/bootstrap` | The account, the memberships, the plan. |
| `GET /api/organizations/{org}` | The organization; `rate_limit_tier`. |
| `GET /api/organizations/{org}/usage` | `five_hour` and `seven_day` windows: `utilization`, `resets_at`. |
| `GET /api/organizations/{org}/rate_limits` | The tier and its model rate limiters. |

### How claude.ai renders the line, for reference

From the web app bundled in the desktop app
(`C:\Program Files\WindowsApps\Claude_2.42304.2.0_x64__pzs8sxrjxfjjc\app\resources\ion-dist\assets\v1\`,
4026 chunks, 153 MB):

- The status row is `JR` → `data-testid="working-activity-row"` → `ov`, with the shimmer dots
  (`leading`), the label, `elapsedSeconds` and the chevron. The clock starts at the earliest step
  that is *not* a narration and *not* an event, so it measures real work.
- A turn that carries a narration renders as a status line rather than prose
  (`form: "status_line"`).
- The recap's step label is `summaries.at(-1)`.
- A second carrier exists: a **text block with `disposition: "narration"`**, the marker arriving on
  `content_block_stop`.
- The client-side fallback, used when the server sends no summary, is `Jce`: the first meaningful
  line of the thinking text, markdown-stripped, capped at 200 characters.

## Decisions

1. **A second transport on the existing Claude engine, not a new engine.** The Claude engine keeps
   its identity, its model catalog and its tool vocabulary.

   The transport is the **account's** choice, not a thread's or a model's: it is a Forge setting
   (`claude_transport`), set in *Settings → General → Behaviour*, and every Claude thread on that
   Forge follows it. A new `EngineId` would mean a new entry in the catalog, the picker, the
   settings and the account probes, for something that is the same engine with a different wire;
   a per-model picker axis would make it a per-thread accident.

2. **The recap is a separate event, not a trimmed prose.** `LiveKind::Narration` publishes the
   provider's line whole and bypasses `reasoning.rs`'s clause-cut, which is the whole point: the
   native line is *"Checking the proper procedure before listing files."*, and trimming it would
   leave *"Checking"*.

3. **The session is a file, not a credential type.** claude.ai's chat API is cookie-authenticated
   and there is no first-party OAuth for it, so the session is a browser session, kept as JSON in
   the engine home at `.claude-ai/session.json`.

4. **The public API stays the default.** The claude.ai path is opt-in per thread, because it is an
   unversioned private API and it puts the model loop on Anthropic's side rather than on the
   user's machine. A thread that asks for it without a signed-in session falls back to Claude Code
   rather than failing.

5. **Forge runs the tools, claude.ai runs the model.** The transport declares the tools, runs
   them here — under Forge's own permissions — and sends the results back. The tools carry
   Claude Code's names and shapes, so the Editor renders them the way it renders Claude Code's.

## What is built

| Piece | Where | Done |
|---|---|---|
| The protocol | `claudeai/transport.rs` | ✓ |
| The recap | `claudeai/decoder.rs`, `LiveKind::Narration` | ✓ |
| The tool runtime | `claudeai/tools.rs` | ✓ Read, Write, Edit, Glob, Grep, Bash (foreground and background), TodoWrite, WebFetch, AskUserQuestion |
| Skills | `claudeai/skills.rs` | ✓ The home's and the project's, listed in the preamble and loaded on demand through `Skill` |
| MCP servers | `claudeai/mcp.rs` | ✓ The launch's servers, the project's `.mcp.json`, stdio and Streamable HTTP, listed as `mcp__server__tool`, run under the same approvals as Bash |
| Subagents | `claudeai/mod.rs` | ✓ `Task` runs a nested claude.ai conversation; its events hang under an `Agent` item and its approvals and questions reach the user |
| Permission rules | `claudeai/rules.rs` | ✓ `settings.json` `allow`/`ask`/`deny` and `defaultMode: acceptEdits` |
| The tool loop | `claudeai/mod.rs` | ✓ Runs the calls, sends the results back |
| Permissions | `claudeai/tools.rs` | ✓ Auto asks for writes and commands; Full runs them |
| Approvals | `claudeai/mod.rs` | ✓ The user answers the item, the run waits |
| Questions | `claudeai/mod.rs` | ✓ `AskUserQuestion` becomes a question item |
| Steers | `claudeai/mod.rs` | ✓ A steer that arrives mid-turn is the next prompt |
| The session | `claudeai/session.rs`, `crates/claude-ai-browser` | ✓ Captured in the user's browser over DevTools, checked by the Forge, kept 0600, cookies written back, renewed by the Editor |
| The command | `ae engine login\|logout claude --claude-ai`, Editor → Claude Code → Account → claude.ai session | ✓ |
| Usage | `claudeai/capabilities.rs` | ✓ The account's windows, read from claude.ai |
| The fallback | `claude.rs` | ✓ No session, no failure: Claude Code runs the turn |
| The setting | `artisan-api/src/transport.rs`, `editor-settings/src/general.rs`, `forge-runs/src/run.rs` | ✓ General → Behaviour, a Forge setting, every Claude thread follows it |
| The path a run keeps | `artisan-api/src/transport.rs`, `forge-runs/src/run.rs`, `claudeai/mod.rs` | ✓ A claude.ai session carries a `claude.ai:` prefix; a run that carries on it stays there |

## What is left

1. **The `disposition: "narration"` carrier is not used.** claude.ai marks a *text block* as a
   narration as well as sending the recap as a delta. Only the delta is read; the text block is
   rendered as an ordinary assistant message, so a turn that narrates shows the line twice.

2. **The claude.ai conversation is not the thread's transcript.** Forge stores the thread; claude.ai
   keeps its own conversation, and only the id is shared. Resuming a thread in the Editor resumes
   claude.ai's conversation, not Forge's, so the two can drift.

3. **Background subagents are not built.** `Task` runs in the foreground; a
   `run_in_background` call is refused with a note. Hooks, slash commands and compaction are
   still Claude Code's, and a background shell lives only as long as its run.

4. **The sign-in is a browser, on the user's device.** A cookie export made the user paste their
   own credentials and went stale within hours. The browser opens on a profile of Artisan's own,
   the user signs in, and the cookies and user agent are read over DevTools; the same profile
   renews the session when Cloudflare asks again. The capture runs where the user is (`ae` or the
   Editor) and the session is handed to the Forge, which may be another machine.

5. **A turn's text is written out with its message, not with the live preview.** The preview is
   dropped when the run ends, so the message has to carry its own text. claude.ai's stream also
   runs the provider's own tools inside one completion: a tool call the provider answered itself
   comes back with a `tool_result` and is not the caller's to run.

6. **claude.ai's accounting is not Forge's.** The usage windows come from claude.ai's subscription,
   not from the API, and Forge's `Usage` events count the tokens claude.ai reported. The two
   windows are read from different sources and do not have to agree. A subagent's usage is not
   added to its parent's.

7. **It is an unversioned private API.** The fields, the event names and the tool contract can
   change without notice. The transport refuses a turn it does not understand rather than
   guessing, and falls back to Claude Code when there is no session; a change in claude.ai's shape
   would show up as a failed turn, not a wrong one.

## How to re-verify

```
CLAUDE_AI_SESSION=/path/to/session.json \
  cargo test -p forge-engines --lib claudeai -- --ignored --nocapture
```

Two live tests: one that a turn carries the provider's recap, and one that a tool call round
trips — the model calls `Read`, Forge runs it here, and the model quotes the file's contents.
