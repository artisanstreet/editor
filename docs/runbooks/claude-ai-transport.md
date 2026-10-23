# The claude.ai transport (Native Claude Mode)

> Plan and open questions: `docs/plans/claude-ai-transport.md`.

Claude Code talks to `api.anthropic.com`, which returns the model's reasoning as prose. Forge
trims that prose into a clause, and the result is the first half of a sentence: `README` for
*"README.md doesn't exist, and those MCP docs tools aren't relevant…"*.

claude.ai's own chat backend returns something better. The provider writes a one-line recap of
what the model is doing and sends it as its own event — the text claude.ai shows above each tool
group. The public API never sends it, and `thinking.display` has no value that asks for it.

So Forge speaks claude.ai's endpoint directly, as a second transport for the Claude engine.

## Turning it on

**Settings → General → Behaviour → Native Claude Mode.** It is the account's choice, not a
thread's, so it is a Forge setting (`claude_transport`, `claude_code` or `claude_ai`) and every
Claude thread on that Forge follows it. The **claude.ai session** row in
**Settings → Claude Code → Account** says where the session stands and signs in or out.

Forge picks the transport per run (`forge-runs/src/run.rs`, `claude_transport`):

| Setting | Session | What runs |
|---|---|---|
| `claude_code` | any | Claude Code. |
| `claude_ai` | kept | claude.ai. |
| `claude_ai` | none | Claude Code, with a notice in the thread saying why. |

**Switching moves every Claude thread.** A session only carries on on the transport it was made
on (a claude.ai session carries a `claude.ai:` prefix). When the transport changes, the run
starts a fresh session and the thread's history is handed to it, the way an engine switch is.

## Signing in

claude.ai's chat API is cookie-authenticated, and Cloudflare in front of it gives its clearance
only to a real browser. So the session comes from one, on the user's own device
(`crates/claude-ai-browser`):

1. Chrome (or Edge, Chromium, Brave) opens on a profile of Artisan's own
   (`<Artisan Street>/claude-ai-browser`), at claude.ai's sign-in page.
2. The user signs in as they would anywhere.
3. The cookies (claude.ai's only) and the user agent are read over the browser's DevTools
   protocol, and the window closes.
4. The session goes to the Forge (`EngineCommand::ClaudeAiSignIn`), which checks it against
   `/api/bootstrap`, finds the account and organization, and keeps it.

From the Editor: **Settings → Claude Code → Account → claude.ai session → Sign in**. From a terminal:

```
ae engine login claude --claude-ai     # sign in in a browser here
ae engine logout claude --claude-ai    # forget the session
ae engine list                         # shows where the claude.ai session stands
```

The browser has to be where the user is, and the Forge may be another machine, so the capture
never runs in the Forge:

- **Windows with Forge in WSL:** Windows' `ae` signs in with Windows' browser and pipes the session
  to the `ae` in WSL (`--session-stdin`), which keeps it.
- **`ae` run inside WSL with no Linux browser:** it runs Windows' `ae.exe … --capture-only` to open
  Windows' browser, and keeps what it prints.

The session is kept in the Claude engine home at `.claude-ai/session.json`, mode 0600 in a 0700
folder, written atomically:

```json
{
  "cookies": [["sessionKey", "..."], ["cf_clearance", "..."]],
  "user_agent": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) ...",
  "org_uuid": "828990a5-…",
  "account": "me@example.com",
  "needs_renewal": false,
  "expired": null
}
```

## Keeping the session alive

- **Cookies rotate.** claude.ai sends a fresh `__cf_bm` (30 minutes) and others in `Set-Cookie`
  on every response. The transport writes them back to the file, merging under a lock so
  concurrent runs do not lose each other's cookies.
- **Browser checks.** When Cloudflare answers with its challenge page (or `cf-mitigated:
  challenge`), the session is marked `needs_renewal`, the turn fails retryably ("claude.ai asked
  for a browser check"), and no request goes to claude.ai until it is renewed. An Editor on a
  device that signed in sees the state in the engine status and renews by itself: the same profile
  opens claude.ai off-screen for a few seconds and the new clearance goes to the Forge. At most
  every ten minutes.
- **Signed out.** A 401/403 that is not a challenge marks the session `expired`; turns fail with
  "sign in again" until someone does.
- **Deleted conversations.** Conversations are temporary, so they stay out of the user's claude.ai
  history, and claude.ai may delete one. A `404 chat_conversation_not_found` on resume clears the
  run's session without delivering the message; Forge runs it again on a fresh conversation, which
  starts from the thread's history.

The Editor's engine status carries `claude_ai`: `signed_out`, `signed_in`, `needs_renewal` or
`expired`.

## What it sends

```
POST /api/organizations/{org}/chat_conversations
    {"name": "Artisan", "is_temporary": true}                 -> 201 {"uuid": ...}

POST /api/{org}/upload   (multipart, field "file")            -> {"file_uuid": ...}

POST /api/organizations/{org}/chat_conversations/{uuid}/completion
    {"prompt": "...", "model": "claude-opus-5-5",
     "thinking_mode": "auto", "effort": "high",
     "rendering_mode": "messages",
     "attachments": [], "files": ["<file_uuid>"], "sync_sources": [],
     "tools": [...], "tool_states": [...]}

POST /api/organizations/{org}/chat_conversations/{uuid}/stop_response
```

- **`effort` gates the recap.** Without it the turn comes back with no `thinking_summary_delta`.
- **`thinking_mode`** is one of `extended`, `standard`, `auto`, `off`.
- **`prompt` must not be blank** (`400 no_prompt_provided`).

The first message of a conversation carries a preamble claude.ai's own system prompt does not:
the working directory, platform and date; which tools run on the user's machine (and that
claude.ai's sandbox tools cannot see it); how tool results come back; the skills installed for
the model, with their descriptions; the MCP servers connected for the run (and any that failed to
start); and the user's CLAUDE.md files (the engine home's, where Artisan's global prompt lives,
then the project's from the repository root down).

The stream is a superset of the Messages API stream, plus `thinking_summary_delta` (published
whole as `LiveKind::Narration`), `message_limit` and `tool_origin` on a tool call.

## The tools

The transport declares the tools and runs them here, under Forge's permissions. Names and input
shapes are Claude Code's (`file_path`, …), so the model calls them as trained and the Editor shows
them as it shows Claude Code's, diffs included: `Read`, `Write`, `Edit`, `Glob`, `Grep`, `Bash`,
`TodoWrite`, `WebFetch`, `Task`, `Skill`, `AskUserQuestion`, and the run's MCP servers' tools as
`mcp__server__tool`.

- **Skills** are the ones Claude Code would see: the engine home's `.claude/skills` (where Forge
  links the enabled ones) and the project's, with the project's winning name collisions. Their
  names and descriptions ride in the preamble's `<available_skills>`; `Skill` loads a skill's
  `SKILL.md` and lists its bundled files, so the model reads a reference with `Read` only when the
  skill says to.
- **MCP servers** come off the launch (`Launch::mcp_servers`): the user's servers and Forge's own
  gateway (`artisan`, and `computer` when computer use is on), plus the project's `.mcp.json`
  files from the repository root down (a nearer file's server of the same name wins). stdio
  servers are started with their command, args and env; remote ones are POSTed to JSON-RPC over
  Streamable HTTP, with the session and protocol headers. Their tools are offered as
  `mcp__server__tool`, and a call under Auto asks like a command does ("Allow always" holds for
  that tool). A server that fails to start is named in the preamble's `<mcp_errors>` and the turn
  carries on without it.
- **Subagents** are `Task`: the call runs a nested claude.ai conversation with the same tools and
  the working directory, whose events appear under an `Agent` item (keys prefixed, items parented
  to it). Its approvals and questions reach the user as usual; its final reply is the tool's
  result. `run_in_background` is refused with a note.
- **Permission rules** are Claude Code's own, read from the engine home's and the project's
  `settings.json`/`settings.local.json`: `deny` always refuses (even under Full), `ask` always
  asks, `allow` runs without asking, and `defaultMode: acceptEdits` runs file changes without
  asking. A rule this transport does not understand never matches, so an unknown `allow` cannot
  widen anything.
- **Approval comes first.** Under Auto, `Write`, `Edit`, `Bash` and MCP calls show a card built
  from the call's input alone; nothing runs until the user allows it, and a declined call never
  runs. "Allow always" holds for the rest of the run (file changes, that exact command, or that
  MCP tool).
- **Results ride in the prompt.** claude.ai does not show `tool_states` to the model (probed: a
  result only there is invisible), so every result goes in the next prompt inside
  `<tool_results><result tool_use_id=… tool=… status=ok|error>…</result></tool_results>`, with a
  line saying it is not the user's message. `tool_states` still closes the calls.
- **Parallel calls.** After a call to one of our tools the stream says nothing more (it only pings
  every 15 s), so the read waits two seconds for further calls, then answers them all in one
  message. Before running those tools, Forge stops the previous server completion and waits for
  confirmation (at most 15 seconds). Closing the event stream alone leaves it running and can
  exhaust concurrent-completion slots after a few tool rounds. A failed stop ends the turn without
  starting another completion. Text before the calls is kept as the turn's commentary.
- **claude.ai's own tools** (`tool_origin` set: its sandbox, web search) are shown with their
  results and never answered by Forge. **Connectors** (an `mcp_server_url`) run on claude.ai
  without asking, so under Auto the transport stops the response and the turn when one starts.
- **Background shells.** `Bash` with `run_in_background` starts the command in its own process
  group and returns a `bash-N` id; `BashOutput` reads what it printed since the last read and
  whether it still runs; `KillShell` stops it and everything it started. Output is kept up to
  256 KB; the run's cancellation ends every shell it started.
- **Limits.** Read/Edit/Grep open files up to 16 MB and skip binaries; Glob/Grep honour
  `.gitignore`, walk at most 200 000 entries off the async threads and return at most 2 000
  matches; output to the model is cut at 256 KB. `Bash` runs in its own process group (a job
  object on Windows), 2 minutes by default and 10 at most; a stop or timeout ends everything it
  started, and the model is told the exit code. `WebFetch` reads at most 2 MB of a page and turns
  HTML into text.
- **Steers** wait for the next message: with the tools' results, or as the next prompt when the
  model answered. Each input is acknowledged (`InputAccepted`) once it is sent, so Forge never runs
  it again.

HTTP 429 and streamed `rate_limit_error` responses do not always mean spent usage. An
`exceeded_limit` with `representativeClaim: concurrents` is a temporary request limit and uses
the short overload retry. The error says "Claude.ai has too many active responses." Only a
named usage quota or an exhausted usage window is reported as spent usage and parks until a
usage reset. An unidentified `exceeded_limit`, even with a reset timestamp, says the provider
did not identify the limit and uses the short retry too. The full limit payload is logged.

## Not on this transport

Background subagents (`Task` with `run_in_background` is refused with a note), hooks, slash
commands and compaction are Claude Code's. A background shell lives only as long as its run, and a
subagent's usage is not added to its parent's.

## Tests

`cargo test -p forge-engines --lib claudeai` covers the decoder, the tool runtime, skills, MCP
(over a fake HTTP server), permission rules, background shells, the session file, claude.ai's
refusals and whole turns against a scripted claude.ai (approval before running, delivery
acknowledged once, parallel calls, steers, deleted conversations, stops, images, a skill loaded on
demand, todos, a subagent's nested conversation, a background shell read and stopped, an allow
rule and a deny rule, and an MCP server's tool called).
`forge-runs` covers the transport choice. The live test needs an engine home with a session:

```
CLAUDE_AI_HOME=/path/to/engine/home \
  cargo test -p forge-engines --lib a_live_turn -- --ignored --nocapture
```

It reads a file through `Read` and checks the model quotes it.
