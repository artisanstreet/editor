# Background terminals in the thread inspector

- Status: Claude and Codex built on branch `background-terminals` (2026-10-08); see Implementation
  status. ACP and Codex unified exec are still open
- Scope: capture every terminal an engine opens, including long-running background jobs
  (`vite dev`, `cargo watch`, test watchers). List them in the right-hand thread inspector. A
  click opens a dialog with the latest output, ANSI colours included.
- Use case: an agent is holding the Vite dev server. You want to see what just errored without
  asking the agent or opening a shell.

## Outcome

Every shell job an engine starts shows up as a row in the inspector's **Terminals** section. That
includes Claude `Bash` (foreground and `run_in_background`), Codex command executions and
unified-exec sessions, and ACP `terminal/create`. A row shows:

- a state glyph: running, exited cleanly, or failed or killed,
- the display name and command,
- a one-line preview of the last output line.

Clicking a row opens a dialog with a live tail of the output. It renders SGR colours, bold and
dim, applies carriage-return overwrites, and treats a clear-screen as a fresh frame, so a Vite
restart shows only the current screen. The output is scrollable through the whole retained
buffer and opens scrolled to the bottom. It follows new output while the user is at the bottom
and stays put once they scroll up; scrolling back to the bottom resumes following. The open
dialog keeps streaming new output.

Background jobs outlive the turn that started them, so the section is scoped to the **thread**,
not to the live run. A dev server that crashes after the turn ended still shows its red exit and
the stack trace.

## What exists today

The UI shell is half built. Capture, transport and colour rendering are missing.

- **Inspector section exists but is unwired.** `ThreadScreen::render_terminals_section`
  (`modules/frontend/src/thread_screen/render.rs:289`) draws a skeleton and the live rows from
  `ThreadScreen.terminals: Vec<TerminalSession>` (`thread_screen/state.rs:138`). `set_terminals`
  is only called from tests. Its doc comment marks click-to-inspect and the tail dialog as gaps.
  Rows aren't clickable.
- **The Agents section is the template.** `thread_agents.rs` is a pure projection over
  `EngineObservationState`. `sync_thread_inspector`
  (`native_application/impl_route_surface.rs:303`) pushes it into the screen with a cheap
  `presents` equality guard. It is gated on `run_controls.is_live_for(thread)`, and that gate is
  wrong for background terminals (see Phase 3).
- **Dialog primitive exists but is unused.** `modules/ui/src/dialog.rs` (`Dialog::new(...)`,
  `on_dismiss`, focus restore) has no frontend caller yet. The context-meter popover
  (`native_application/impl_context_meter.rs`) shows the pattern for hosting an overlay: an open
  flag on `NativeApplication`, toggled on click and cleared on dismiss.
- **There is no ANSI rendering.** `terminal_presentation.rs:102` (`present_terminal_output`)
  *strips* escape sequences and resolves `\r`. The transcript shows only the command line, never
  the output (`conversation_surface/render_blocks.rs:1097`).
- **The domain type is too thin.** `TerminalActivityInput`
  (`modules/domain/src/observation/tooling.rs:78`) carries `activity_id`, `channel`, `command`,
  `shell`, an `output` chunk (8 KB cap), `exit_code` and `state ∈ {Started, Output, Completed,
  Failed}`. There is no background flag, no Running/Killed state, no engine job id and no cwd.
  Every chunk lands in the append-only `observation_ledger`.
- **Each engine has a gap:**
  - **Claude** (`engine_owner/claude/tools.rs:109-258`, `protocol.rs:551-585`): only `Bash` is a
    terminal row, and Claude never streams output. `run_in_background` is ignored. A background
    Bash shows as "completed", with the "running in background" text as its output.
    `BashOutput`/`KillShell` are generic tools. `task_*` system events collapse to
    `SubagentLifecycle`, and `background_tasks_changed` drops `local_bash` on purpose. Bash calls
    inside subagents never become terminal rows.
  - **Codex** (`engine_owner/codex/protocol.rs:972-1245`, `adapter.rs:369-955`): streams
    `commandExecution` output deltas, and each 8 KB slice becomes a ledger row. `processId` and
    `terminalInteraction` (unified exec, which Codex calls "background terminals") are not parsed.
    `experimentalApi: false` is sent at `protocol.rs:1721`.
  - **ACP** (`acp/runtime.rs:536`): no `terminal` client capability is advertised, so agents fall
    back to their own internal shell. `tool_call` updates are not projected at all.
  - **OpenCode**: no shell rows.
- **Claude already writes background output to a file.** The pinned Claude Code generation answers
  a background Bash with `Command running in background with ID: <id>. Output is being written to:
  <path>.` Its `task_started` system event carries `task_id`, `tool_use_id`, `description` and
  `is_backgrounded`. `task_notification` carries `task_id`, `tool_use_id`, `status` and `reason`.
  The engine and the Forge share a host, so the Forge can tail that file. This is the only way to
  see a Claude background job's output without the model polling `BashOutput`.

## Principles

1. **Lifecycle goes to the ledger, output goes to a bounded live buffer.** A dev server can print
   megabytes over hours. Putting each chunk in the append-only ledger as an observation (what Codex
   does today) grows the database without bound, and nobody replays it. The ledger records the
   facts that matter for history: started, backgrounded, exited with code N, killed. Output lives
   in a per-terminal ring buffer in the Forge, about 256 KB of raw bytes, which the Editor reads
   and subscribes to. When the buffer drops old output, the dialog says "earlier output
   truncated". Transcript rendering of foreground output keeps its existing 8 KB ledger excerpt.
2. **Keep the raw bytes and interpret them at display time.** The Forge never strips or rewrites
   escape sequences. The Editor parses them once per frame from the buffered tail, so a sequence
   split across chunks is never misread. This is the same rule `append_terminal_output` already
   documents.
3. **Identity comes from the engine.** The terminal id is the engine's own handle: Claude's
   background task id or `tool_use_id`, Codex's `processId` or item id, the ACP `terminalId`. It is
   scoped by thread. Follow-up calls (`BashOutput`, `KillShell`, `terminalInteraction`, `task_*`
   events) attach to the existing row and never create a new one.
4. **The inspector holds terminals, not commands.** The section lists background jobs plus any
   foreground command that is still running. A finished foreground `ls` leaves the panel, since the
   transcript already shows it. Background jobs stay listed after they exit, because "what crashed"
   is the main question this panel answers.

## Design

### Domain and protocol

Add a thread-scoped terminal model next to the observation. Don't overload
`TerminalActivityInput`.

```text
TerminalId          engine-native handle, newtype, unique per thread
TerminalLifecycle   Opened { command, cwd, shell, background, attribution }
                    Backgrounded            (a foreground command moved to background)
                    Exited { code }         | Killed { reason } | Lost   (engine gone)
TerminalTail        { terminal, bytes, first_offset, next_offset, truncated }
```

- `TerminalLifecycle` becomes a new observation kind in `modules/domain/src/observation` and the
  capnp union, and is committed to the ledger, so history and reconnects rebuild the panel.
- `TerminalTail` is **not** an observation. It is a Forge→Editor read (`terminal.tail(thread, id,
  since_offset)`) plus a push subscription (`terminal.output(thread, id, offset, bytes)`). Offsets
  are byte positions in the job's whole output, so the Editor can splice and detect gaps after a
  reconnect. Schema goes in `modules/protocol/schema/artisan.capnp`.
- Codex `Output` deltas move from ledger rows to the live buffer. The existing
  `TerminalActivity` Started/Completed rows stay as they are for transcript rendering.

### Forge: terminal registry (`modules/backend/src/terminals/`)

There is one registry per thread. It maps `TerminalId` to `{ lifecycle, RingBuffer, subscribers,
source }`, where `source` is one of:

- **`EngineStream`**: the engine pushes deltas (Codex output deltas and ACP-owned processes).
- **`TailFile(path)`**: Claude background jobs. A watcher (`notify`, or a 250 ms poll fallback)
  reads appended bytes into the ring. It stops on `task_notification` with a terminal status, on
  `KillShell`, or when the engine process exits. In those cases it reads to EOF once, then stops.
- **`Owned(child)`**: ACP terminals that the Forge spawns itself (Phase 5).

The registry lives with the engine owner and must not outlive it. When the engine process exits,
every non-final terminal becomes `Lost`. Background shells die with the engine (see memory "Background
agents die when stdin closes"), and the panel has to say so rather than show a stale "running".

### Engine capture

- **Claude**
  - Parse `run_in_background` from the `Bash` input. Parse `<id>` and `<path>` from the tool
    result, with a regex anchored on the exact sentence plus a fixture test pinned to the
    generation.
  - Use `task_started`/`task_notification` with `tool_use_id` as the structured confirmation and
    exit signal, and stop dropping `local_bash` in `background_tasks_changed`.
  - Map `BashOutput` and `KillShell` onto the existing terminal: `KillShell` gives `Killed`.
  - Foreground `Bash` stays Started→Completed. It has no live output because Claude doesn't stream
    it, and the row appears only while the command runs.
  - Route Bash calls inside subagent child frames too, attributed to the subagent.
- **Codex**
  - Feed `item/commandExecution/outputDelta` into the ring buffer instead of ledger rows.
  - Turn on `experimentalApi` for the unified-exec fields only if the pinned app-server version
    needs it for `processId`/`terminalInteraction`. Verify against the generated schema of the
    pinned binary first.
  - Map a `commandExecution` that ends with a live `processId` to `Backgrounded`, and later
    interactions or exit to the same row.
- **ACP**: Phase 5. Advertise `clientCapabilities.terminal: true` and implement `terminal/create`,
  `terminal/output`, `terminal/wait_for_exit`, `terminal/kill` and `terminal/release` with
  `Owned` sources. We then own the process, so we decide whether it gets a pty.

### Editor

- **Projection.** A new `thread_terminals.rs` mirrors `thread_agents.rs`: lifecycle facts in,
  `Vec<ThreadTerminalEntry>` out. Each entry has id, display name, command, state, exit code,
  attribution and a last-line preview. Replace `TerminalSession` in `ThreadScreen` with the entry
  type and keep the `presents` equality guard.
- **Sync.** `sync_thread_inspector` pushes terminals for the open thread **regardless of run
  liveness**, while Agents and Checklist keep their run gate.
- **Ordering.** Running terminals come first, newest first. Exited, killed and lost terminals
  follow under a muted "Exited · N" disclosure, collapsed by default. A failed exit is tinted with
  the error colour, and the icon uses the text colour muted to about 80% (memory "Coloured elements
  colour their icons"). Hover gets the hover fill only.
- **Row click.** Clicking a row sets `terminal_dialog: Option<(ThreadId, TerminalId)>` on
  `NativeApplication` and opens the dialog. Opening it subscribes to the tail and closing it
  unsubscribes. Every row is a `cached(style)` entity (memory "GPUI cached views are the frame
  budget"), and an output tick notifies only the dialog entity, never the inspector. The inspector
  preview line is throttled to lifecycle changes plus about one update per second.
- **Dialog** (`ui::Dialog`, about 70% of the window, hosted on the app root)
  - Header: display name, mono command, cwd, state pill, exit code and uptime.
  - Body: a virtualised list of rendered lines in the mono font.
  - Footer actions: Copy output (plain text, escapes stripped) and Jump to bottom. A Kill button
    exists only for `Owned` terminals. We can't kill a Claude or Codex job safely behind the
    engine's back.

### ANSI rendering (`modules/ui/src/terminal_text/`)

The renderer is a line model, not a full terminal grid. That covers dev servers, compilers and
test runners. Full-screen TUIs (`htop`, `vim`) are out of scope, and the dialog shows a hint for
them.

- Parse with the `vte` crate, the parser Alacritty and Zed use. It is small and handles split and
  malformed sequences correctly.
- Supported:
  - SGR: reset, bold, dim, italic, underline, inverse, the 16 colours plus bright, 256-colour and
    truecolor.
  - `\r` overwrite, `\x1b[K`/`\x1b[2K` (erase line), `\x1b[nA`/`\x1b[nF` (cursor up, used by
    spinners and progress bars).
  - `\x1b[2J`/`\x1b[3J`/`\x1b[H` clear screen: everything before it collapses into "earlier
    output" and the view starts fresh. Vite does this on restart.
  - OSC 8 hyperlinks become clickable runs. Other OSC and DCS sequences are dropped.
- Output is `Vec<TerminalLine { runs: Vec<(Range, TerminalStyle)> }>`, mapped to GPUI
  `StyledText` highlights. The 16 base colours come from a new `ArtisanTheme.terminal` palette
  (light and dark), and 256-colour and truecolor values are adjusted for contrast against the
  dialog background.
- Parsing is incremental. A parser state per open dialog consumes only new bytes. It re-parses
  from the buffer start only after a gap or truncation.
- `present_terminal_output` keeps serving plain-text callers (copy, preview line). It is
  reimplemented on the same parser, so stripping and rendering cannot disagree.

### Colours have to exist first

Claude runs Bash without a TTY, so `picocolors`/`chalk` (Vite, most JS tools) and `cargo` turn
colour off. The plan only produces colour if the job emits it:

- Codex unified exec runs on a pty, so it gets colour.
- Forge-owned ACP terminals can use a pty (`portable-pty`) or set `FORCE_COLOR=1` and
  `CLICOLOR_FORCE=1`.
- Claude: setting `FORCE_COLOR=1` in the engine environment would also put escape codes into
  foreground output that the model reads, which costs tokens and adds noise. Phase 0 checks what
  Claude Code itself sets for background shells. If it sets nothing, ship Claude without colour (it
  still renders plain) and decide separately.

## Implementation status

Built on `background-terminals` (2026-10-08). Where it differs from the design above, the
difference is listed here, and this section wins.

- **Engines own one turn, so jobs do too.** Claude (`claude -p`) and Codex (`app-server`) are
  spawned for one turn and torn down after it. Claude Code also kills its own background shells
  when input closes. A background dev server therefore lives as long as the agent's turn. The
  board keeps the job and its last output after that, marked `Ended with the agent's turn`. If
  jobs should outlive the turn, the Forge has to own the process (the ACP terminal capability,
  phase 6) or keep an engine session alive between turns.
- **No ledger observation kind.** All engine processes are gone after a Forge restart, so a
  terminal never needs replaying. Lifecycle and output both live on the in-memory
  `TerminalBoard` (`modules/backend/src/thread_terminals.rs`), the way `LiveThinkingBoard` works.
  Engines report changes as `EngineObservation::Shell(ShellJobRow)`. The dispatcher applies them
  and never commits them, and ends a run's jobs that are still running when the turn settles.
- **Wire.** `Event::ThreadTerminals` is event union member `@19`. Each push lists every job of the
  thread, oldest first, and carries only the output that subscriber does not have yet, as
  `outputStart` plus `Data` bytes. Activation sends the full retained window.
- **Retention.**
  - Each job keeps the newest 256 KiB of output.
  - Each thread keeps at most 32 jobs; stopped jobs are dropped first.
  - The board keeps at most 64 threads; idle threads are dropped first.
  - A foreground command leaves the board when it finishes, because the transcript already shows
    it. A background job stays after it stops.
- **Claude.**
  - `Bash` opens a job.
  - The result's `tool_use_result.backgroundTaskId`, plus the output path in its text, starts the
    `ShellTail` follower (`engine_owner/shell_tail.rs`, polls every 250 ms).
  - `task_*` reports for `local_bash` settle the job, with the exit code taken from the summary.
    They no longer register as subagents, which they wrongly did before.
  - Foreground Bash output is not streamed, so a running foreground command has no output yet.
  - Bash calls inside subagents are not captured yet.
- **Codex.**
  - `commandExecution` start and completion open and settle jobs.
  - `outputDelta` goes to the board and is no longer written to the ledger. The completed item
    still carries the bounded aggregate.
  - Unified exec (`processId`) is not wired.
- **Editor.**
  - `ThreadTerminalsView` (`modules/frontend/src/thread_terminals.rs`) mirrors the pushes into one
    `TerminalScreen` per job, fed only new bytes. It draws the dialog: a GPUI `list` in
    `FollowMode::Tail`, which opens at the bottom, follows while the reader is at the bottom,
    stays put after they scroll up, and offers "Jump to latest".
  - Output re-renders only the dialog. The application re-renders only when rows change.
  - Rows list running jobs first and show a state mark in the Agents section's language. There
    is no collapsed "Exited" group yet.
  - The legacy, unwired `terminal_presentation` module is removed.
- **Not done.** ACP terminals (phase 6), Codex unified exec, Bash calls inside subagents, the
  colour question for Claude (open question 2), and a manual run against a live Vite server.

## Phases

0. **Probe (no product code).**
   - Record fixtures from the pinned Claude generation: a background `vite`, `BashOutput`,
     `KillShell`, `task_*` events, and what `FORCE_COLOR`/`TERM` the job sees.
   - Do the same for the pinned Codex app-server: `processId`, `terminalInteraction`, and whether
     `experimentalApi` is needed.
   - Confirm a Claude background shell dies when the engine process exits.
1. **ANSI line renderer in `modules/ui`** with unit tests on real Vite, cargo and vitest captures.
   It can ship on its own: the transcript can use it later for foreground output.
2. **Domain, protocol and Forge registry.** Add the `TerminalLifecycle` observation, the tail
   read and subscription, the ring buffer, and `Lost` on engine exit. Move Codex output deltas off
   the ledger.
3. **Editor panel.** Add the `thread_terminals` projection, sync without the run gate, ordering,
   exited disclosure and cached rows. Codex is wired end to end at this point.
4. **Dialog.** Live tail, auto-follow, copy, truncation notice.
5. **Claude capture.** File tailing, `BashOutput`/`KillShell`/`task_*` mapping, subagent
   attribution.
6. **ACP terminal capability.** Forge-owned processes, then `tool_call` projection for ACP shells.

Phases 1 and 2 can run in parallel. Doing Codex before Claude proves the pipeline on the engine
that already streams.

## Verification

- Unit tests:
  - the parser on recorded captures, including split escapes at every byte offset of a fixture,
  - ring-buffer offsets and gap detection,
  - Claude result-text parsing, pinned to the generation fixture.
- Headless app test: open a thread with a fixture-fed registry, click the row, assert the dialog
  lines and styles through debug selectors.
- Manual: have each engine run `npm run dev` in a Vite project, break an import, and check that
  the red Vite overlay error shows in the dialog within one second. Then end the turn, break it
  again, and check that the panel still updates. Kill the engine and check that the row says Lost.
- Builds: follow memory "Keep builds light while the Editor is in use". Attribute clippy and test
  failures against the red master baseline (memory "Master quality gate is red").

## Open questions

1. Should finished foreground commands also stay in the panel (full history), or only running
   ones? The plan says only running ones, to keep the panel a "what's alive" view.
2. Should Claude get colour through `FORCE_COLOR` in the engine environment, given the
   token-noise cost? Decide after the Phase 0 probe.
3. Should the user be able to stop a job the engine owns? For Claude this means asking the model,
   or sending a steer "kill shell X". Out of scope for v1.
