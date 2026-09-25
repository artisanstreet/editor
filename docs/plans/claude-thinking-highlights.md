# Claude thinking highlights: reverse-engineering notes and integration plan

- Status: research baseline retained; integration plan revised 2026-09-25; implementation not started
- Research frozen: 2026-09-25
- Scope: how the Claude app produces its one-line thinking label, what the Claude Code CLI supports, and how to bring it to Artisan
- Evidence baseline: Claude desktop app 1.40609.1 (MSIX `Claude_2.9939.2.0_x64__pzs8sxrjxfjjc`), embedded Claude Code 2.1.255, claude.ai web build `b1fcb81587`
- Caveat: `highlights` and the `summaries` wire field are internal and undocumented. Everything here is inferred from shipped client code and must be re-verified against the exact pinned CLI before product reliance.

## Outcome

Claude sessions in Artisan should show the same one-line "thinking highlight" the Claude app shows (for example, `Recommending a modern tech stack for a SaaS product`), flowing through the existing reasoning-observation pipeline that Codex already populates. The display prefers a server highlight when available, otherwise a first-line label derived from public summarized thinking, otherwise ordinary status narration. The first release requests `summarized`; hosted `highlights` is a subsequent, evidence-gated optimization. This preference does not imply that both artifacts arrive in the same response.

## Part 1 - Reverse engineering

### 1.1 Two different artifacts

The word "thinking summary" refers to two distinct things in Anthropic clients:

| Artifact | Where seen | Source |
| --- | --- | --- |
| Short one-line label plus duration (`Recommending a modern tech stack for a SaaS product · 9s`) | Collapsed thinking header in the Claude app / claude.ai | Server-provided "highlight": a short title per stretch of thinking |
| Multi-paragraph "Thought" prose | Expanded thinking view, Claude Code `ctrl+o`, T3 Code `Thought` blocks | Server-provided prose summary (`display: "summarized"`) |

Both come from the server. No client, and no public API mode, exposes raw chain-of-thought.

### 1.2 How the research was done

The desktop app is a Microsoft Store (MSIX) Electron package. `WindowsApps` is ACL-locked from WSL, so the bundle was copied out via PowerShell and extracted:

```sh
powershell.exe -NoProfile -Command "Copy-Item -Recurse -Force \
  'C:\Program Files\WindowsApps\Claude_2.9939.2.0_x64__pzs8sxrjxfjjc\app\resources\app.asar' \
  'C:\Users\sander\AppData\Local\Temp\claude-app.asar'"
npx @electron/asar extract claude-app.asar claude-app
```

Runtime facts from `%LOCALAPPDATA%\Claude\logs\main.log`:

- app version `1.40609.1`, packaged Electron, node `24.18.1`;
- loaded `https://claude.ai` with `web commit 47e0de79163d641f5cafbd65812c1799ba3450c0`, `desktop commit 69aac03faa72b798ca63e9eb96057c1b2ade979e`;
- embedded Claude Code `[CCD] Initialized with version 2.1.255`.

The chat UI itself is remote: the app loads the claude.ai web app. Its static bundles are public and were downloaded from the asset CDN named in the homepage HTML (`assets-proxy.anthropic.com/claude-ai/v2/assets/v1/*.js`, 66 chunks, ~19 MB). The Claude Code CLI is a 242 MB native binary whose embedded strings were extracted for analysis.

### 1.3 The short label pipeline (claude.ai bundles)

The rendering logic lives in `shared-12-4P3rNZ7K.js`; the text helpers live in `shared-11-CETOUHfJ.js`.

Thinking blocks carry a `summaries` array alongside `thinking`. The label is derived as follows (minified names preserved):

```js
// shared-12: non-empty summaries, last one wins
function Vp(e){ return !e.thinking?.trim() && (e.summaries?.length ?? 0) > 0 }
function Hp(e){ return (e.summaries ?? []).filter(s => Dl(s.summary ?? "").trim() !== "") }
function Dg(e){ return Hp(e).at(-1)?.summary.trim() }

// shared-11: first meaningful line, markdown stripped, truncated to 200 chars
function oO(e){ let t = e.trim().replace(eO,"").replace(tO,"").replace(nO,"")
                 .replace(rO,"").replace(iO,"$1").replace(aO,"").trim();
               return ie(t).trim() === "" ? "" : t }
function sO(e){ return oO(e) !== "" }
function cO(e){ return Jd(e, sO)?.line }              // first non-empty line
function lO(e){ let t = cO(e); return t === undefined ? undefined : oO(t) }
function uO(e){ /* truncate at 200 chars with U+2026 ellipsis */ }
function dO(e){ let t = lO(e); return t === undefined ? undefined : uO(t) }

// shared-12: the block builder
let { summary, firstContentLine, words } =
  o.type === "thinking" ? th(o)
                        : { summary: undefined, firstContentLine: undefined, words: undefined };
// th(e) = { summary: Dg(e), firstContentLine: lO(e.thinking), words: Dg(e) ?? truncate200(firstLine) }

O.push({
  key: `thinking-${a}`,
  blockIndex: a,
  label: words ?? (running
    ? formatMessage({ defaultMessage: "Thinking" })
    : formatMessage({ defaultMessage: "Thought about it" })),
  ...(words === undefined ? { placeholder: true } : {}),
  ...(duration === undefined ? {} : { duration }),
  state: running ? "running" : "done",
});
```

Duration formatting is in `shared-11` (`"Thought for {seconds}s"`, `{minutes}m {seconds}s`, `{hours}h ...`); the collapsed chip form `9s` uses the `{seconds}s` string from `shared-19-Yi-rkk6b.js`.

Consequences:

- The one-liner is **not** a separate model call made by the client. It is the server's `summaries[].summary` text, picked and trimmed client-side.
- If the server sends no summaries, the client derives a label from the first meaningful line of the prose summary and truncates it to 200 characters.
- If neither exists, the label is `Thinking` (running) or `Thought about it` (settled).

### 1.4 How the server is asked for summaries (Claude Code CLI)

`claude --thinking-display <display>` controls the mode. Help text: "How thinking content appears in the response". The CLI's own enum and schema:

```js
QNe = ["summarized", "omitted", "highlights"]
Smo = { "": true, summarized: true, omitted: true, highlights: true }
```

Semantics:

| Value | Returns |
| --- | --- |
| `omitted` | Thinking blocks with empty text; `signature` only |
| `summarized` | Multi-paragraph prose summary in `thinking` |
| `highlights` | One short title per stretch of thinking, in `summaries` |
| `updates` (beta) | Progress lines between tool calls; beta header `thinking-display-updates-2026-08-18` |

The authoritative `highlights` description, verbatim from the binary:

> 'highlights' returns one short title per stretch of thinking instead of a prose summary. The API accepts it only from Claude Code sessions that Anthropic hosts; if the API rejects it, the session sends 'omitted' (no thinking text) in its place from then on. A request for 'highlights' fails, and changes neither the budget nor the display, after such a rejection, on Amazon Bedrock, Google Vertex AI or another provider without Anthropic's first-party beta features, when experimental betas are off (`CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS` or organization policy), or on a Claude 3 model (except on Microsoft Foundry).

### 1.5 Hosted eligibility: there is no client-side check

The CLI does not test "am I hosted" before sending. It sends `display: "highlights"`, and reacts to rejection:

```js
// rejection detector: HTTP 400 + invalid display value
function pbe(e){
  return e instanceof APIError && e.status === 400 &&
    /thinking\.(adaptive|enabled)\.display: Input should be /.test(e.message)
}

// retry handler (error reducer)
if (display === "highlights" && pbe(err)) {
  markThinkingHighlightsRefused();                       // process-wide latch
  log("[thinking] server rejected thinking.display highlights; asking for omitted for the rest of this process and retrying.");
  telemetry("tengu_thinking_display_highlights_rejected_retry");
  return "retry:thinking-display-highlights";
}

// request build: once latched, downgrade silently
Zg = r.display === "highlights" && isThinkingHighlightsRefused() ? "omitted" : r.display;
gd = { type: "adaptive", display: Zg };
```

The latch lives on the host request latches (`markThinkingHighlightsRefused` / `thinkingHighlightsRefused`) and is process-scoped, not persisted.

Defaults chosen by the CLI (`RPr`/`APr`): explicit flag wins; interactive sessions use `summarized` when the `showThinkingSummaries` setting is on; non-interactive `text`/`json` output defaults to `omitted`. `showThinkingSummaries` is documented in settings as "Request API-side thinking summaries and show them in the conversation and in the transcript view (ctrl+o)".

The SDK control request can change the mode mid-session:

```json
{ "subtype": "set_max_thinking_tokens", "max_thinking_tokens": null, "thinking_display": "highlights" }
```

Schema (from the desktop bundle): `thinking_display: enum(["summarized","omitted","highlights"]).nullable().optional()`. If the mode is unavailable, the control response is:

> `set_max_thinking_tokens: thinking_display "highlights" is not available in this session, so neither the budget nor the display was changed. Send the request again with "summarized", "omitted", or no thinking_display. Possible causes: the API provider, the model, experimental betas being off, or the API having rejected "highlights".`

If the terminal user set the display themselves (`--thinking-display` or `showThinkingSummaries`), control overrides for display are refused: "budget applied; thinking_display not applied, the display stays X because the terminal user set it...".

### 1.6 The desktop app's on-demand flip (CCD)

The desktop app does not request prose summaries for every turn. It spawns Claude Code with `--thinking-display omitted` and flips to `summarized` only while the user has the thinking view open (`modules/backend/src/engine_owner/claude/...` equivalent in the Electron bundle is `.vite/build/index.chunk-7Eg3g-H9.js`):

- feature flags: `summarizedThinking`, `thinkingSummaryOnDemand` (default true), `thinkingDisplaySupported`, `thinkingDisplayControlSupported`;
- spawn args: `DD(e)` sets `thinking-display: "omitted"` when on-demand is active and the user does not want summaries, otherwise `"summarized"`;
- reconcile: `vk(e)` → on want, `gk(e, "summarized", "view_open")`; on close, debounce (`view_close_debounce`) then `gk(e, "omitted", ...)`;
- push: `gk` calls `query.setMaxThinkingTokens(budget, display)` (the SDK control above) and logs `[CCD] thinking display → <value> (<reason>)`;
- telemetry: `desktop_ccd_thinking_display_flip` with `{to_display, trigger, mid_turn, is_ssh}`;
- the desktop never sends `highlights`; highlights is what the hosted claude.ai surface uses for its collapsed label.

### 1.7 Public API gap

The public Anthropic API docs describe only `display: "summarized"` and `"omitted"`, plus the beta `"updates"` (header `thinking-display-updates-2026-08-18`). The public API never returns raw chain-of-thought; summaries are produced by a different model, and billing covers the full thinking tokens, not the summary. `highlights` and `summaries` are not part of the public contract, which is why the CLI degrades on rejection.

### 1.8 Reproduction algorithm for any client

To reproduce the Claude app's collapsed header without internal access:

1. Request `thinking: { type: "adaptive", display: "summarized" }`.
2. On each thinking block, take the last non-empty `summaries[].summary` if present.
3. Otherwise derive a label from `thinking`: strip code fences, headings, list markers, blockquotes, link syntax, emphasis; pick the first non-empty line; collapse whitespace; truncate at 200 characters with `...`.
4. Fall back to `Thinking` while streaming and `Thought about it` when settled.
5. Compute duration from the block's start/stop timestamps and format `Ns` or `Thought for Ns`.

Codex parity note: OpenAI exposes the equivalent directly (`reasoning: { summary: "auto" | "concise" | "detailed" }` returns `reasoning` items with `summary_text` parts). Anthropic's nearest public equivalent is `summarized` plus client-side reduction; `highlights` is the exact server-side equivalent and is gated.

## Part 2 - Integration plan for Artisan

### 2.1 Decision and delivery boundary

Use Part 1 as the reverse-engineered baseline. Preserve the findings about `highlights`, `summaries`, the CLI rejection latch, and the desktop control protocol. Do not repeat the binary investigation as a prerequisite for implementation. Capture runtime frames to settle the remaining transport and eligibility questions.

Deliver in two releases:

1. **Public-summary label:** request `summarized` on a verified supported CLI, carry public summary text through the existing reasoning observations, and derive a short Claude label in presentation code.
2. **Hosted highlights:** enable `highlights` only for a verified eligible execution context, prefer its server-authored titles, and select `summarized` on subsequent launches when highlights cannot be used.

The first release does not depend on hosted access, add a model-settings selector, change the database schema, or implement live display controls. It includes the formatter and native verification. `summarized` requests readable output when thinking occurs; it cannot guarantee that a model will think or return useful text on every turn.

### 2.2 Existing architecture and concrete constraints

| Responsibility | Existing implementation | Required change |
| --- | --- | --- |
| Certified launch | `modules/native_engine/src/claude.rs`, `claude/probe.rs` | Carry a separately verified display capability; retain the existing continuation minimum |
| Turn execution | `modules/backend/src/engine_owner/operation/claude.rs` | Select effective display before spawning; preserve one finite process per turn and native resume |
| Launch arguments | `modules/backend/src/engine_owner/claude/launch.rs` | Append a supported explicit display value through the managed launch policy |
| Frame decoding | `modules/backend/src/engine_owner/claude/protocol.rs` | Decode thinking alongside text and usage; retain content-block identity |
| Observation mapping | `modules/backend/src/engine_owner/claude/adapter.rs` | Emit existing reasoning delta/completed observations through the current owner |
| Shared reasoning contract | `modules/domain/src/observation/conversation.rs` | Reuse public-summary observations and their bounds |
| Durable transport | observation commit, database repository, protocol codec | Reuse unchanged unless a demonstrated contract gap requires a separate decision |
| Frontend pairing | `modules/frontend/src/engine_observation_state/state.rs` | Reuse append-delta and authoritative-completion behavior |
| Scene selection | `modules/frontend/src/conversation_scene/build.rs` | Keep reasoning scoped to the correct run and turn |
| Status rendering | `modules/frontend/src/conversation_surface/contract.rs`, `render_sections.rs` | Apply Claude label policy consistently to copy and visibility decisions |
| Existing text reduction | `modules/ui/src/inline_code_text.rs::summary_line` | Preserve Codex policy; Claude needs first-line reduction without sentence punctuation |

Implementation facts that affect the design:

- The backend module declaration file is `modules/backend/src/engine_owner/claude.rs`, not `claude/mod.rs`.
- `decode_assistant` currently returns one event and prioritizes text over thinking. A mixed content frame must project both; extracting the existing thinking branch alone does not fix this.
- Streaming currently drops content-block boundaries and thinking payloads. Message identity alone cannot distinguish multiple thinking stretches around tools.
- The frontend appends deltas and replaces their text when a completion carries authoritative text. Exploit this reconciliation instead of appending a buffered snapshot twice.
- `summary_line()` prefers the newest headline or a finished sentence. An unpunctuated title can disappear through this formatter. Do not reuse it unchanged for Claude.
- The current scene intentionally removes summaries from settled status rows. This release preserves that lifecycle. A persistent collapsed thinking history would be a separate product change.
- Artisan executes one finite Claude process per turn and reopens provider state with `--resume`. A next-turn display change can use new launch arguments; it does not require an in-process control request.
- File-size enforcement compares against the allowlist, not the current working size. Already oversized touched files must shrink to their allowed size, not merely avoid growth. Recheck actual counts before editing; never raise allowances to land this feature.

### 2.3 Text, identity, and lifecycle contract

**Text provenance.** In verified `summarized` mode, `thinking` is public summary prose and may enter reasoning observations. In verified highlights mode, only captured, recognized `summaries` text supplies the label. `signature`, `signature_delta`, redacted payloads, and unknown private fields never become observation text or diagnostic payloads. Public thinking stays separate from the assistant answer. Treat unknown display semantics as unavailable rather than guessing from non-empty text.

**Identity.** Maintain one reasoning item per provider message and thinking content-block index, scoped to the Artisan run and turn. Use the same item identity for streamed fragments and the corresponding buffered block. Use frame sequence and fragment index for observation deduplication, not as the identity of the reasoning item itself. Capture must establish how buffered blocks map back to streamed indexes before finalizing this mapping.

**Frame projection.** Allow one decoded frame to carry its ordered content events plus one usage sample. Preserve existing text deduplication and phase attribution. A frame containing thinking, text, and tools must not lose any supported observation or count usage repeatedly. Keep this in the existing decoder/owner flow, without a second stream consumer.

**Lifecycle.** Open a tracked phase on a recognized thinking block, append public prose deltas, and complete it at its verified boundary. Reconcile the authoritative buffered text without duplicating streamed text. Specify and fixture-test whether the buffered frame supplies final text after block stop; do not emit conflicting completions merely to accommodate both. Cancellation, failure, and EOF clear transient state and preserve the actual run outcome. A signature-only block and a tool-only turn are valid, not parse failures.

**Scope.** Forwarded child frames retain their child attribution and never update the root reasoning label. Resume creates fresh run-local tracking while preserving the provider session. A previous run's label must not become the current run's thinking status accidentally.

**Bounds.** Respect `OBSERVATION_DELTA_MAX_BYTES`, `OBSERVATION_MESSAGE_MAX_BYTES`, `OBSERVATION_SUMMARY_INDEX_MAX`, frame bounds, and identifier limits. Fragment on UTF-8 boundaries. Bound in-memory accumulation as well as individual deltas. An oversized final summary may settle without replacement text, following the Codex contract; do not invent truncated canonical prose. The 200-character limit applies only to displayed labels.

### 2.4 Packet A: runtime fixtures and launch capability

Capture `summarized` first using the pinned CLI and Artisan's actual launch path. Include stream-JSON input, partial output, permission transport, forwarded children, profile, and resume behavior. Use a scratch project and prompts with no external effects. Record exact CLI version, executable identity, model, provider/auth category without credentials, argv, and capture date.

A fixture capture is complete when it includes:

- A thinking block with streaming deltas and its buffered assistant frame.
- A response with reasoning plus text, and a tool cycle with multiple thinking stretches.
- A start and a native resume, including message and block identities.
- Empty/omitted thinking, a no-thinking turn, and representative interruption frames. Constructed edge fixtures must be labeled separately from actual captures.

Store small sanitized fixtures under `tests/fixtures/claude/` and a capture manifest beside them. Remove credentials, local private content, and opaque signatures while retaining field presence and event ordering. Retain the Part 1 artifact identifiers and versions as provenance; record hashes of available research artifacts without committing entire vendor bundles.

Use the captured version as the initial known-good display version. Do not infer the first supporting release from the research binary's version. Keep `CLAUDE_MINIMUM_CLI_VERSION` and continuation gating unchanged. The certified launch capability should distinguish support for the flag from eligibility for hosted highlights. Below the verified display floor, omit the new flag and keep ordinary status behavior.

Acceptance: replayable summarized fixtures establish the transport mapping, and launch tests cover supported and older binaries. Failure to access hosted highlights does not block Packet B.

### 2.5 Packet B: complete public-summary implementation

1. Add backend-local display policy and capability resolution. Default managed supported launches to `summarized`; unsupported launches retain existing arguments. Avoid adding a persisted `ClaudeSelection` field solely for an internal policy. If Artisan later exposes an explicit display override, define its precedence and persistence in that change. Do not promise to detect arbitrary external CLI configuration that Artisan does not currently model.
2. Extract thinking parsing and tracking into cohesive modules under the existing Claude backend module. Reduce the touched frozen files to their allowances. Change frame projection to preserve mixed content and usage once, with minimal delegation in the existing decoder and adapter.
3. Emit the existing `ReasoningSummaryDeltaObservation` and `ReasoningSummaryCompletedObservation` through the normal observation sink. Keep full public prose in those observations. Reuse dispatch, storage, wire encoding, and frontend pairing.
4. Add a pure Claude label reducer at the presentation layer. Thread the resolved engine/presentation policy through scene and status copy so the renderer, accessibility copy, and paint/visibility decisions use the same result. Preserve the existing Codex reducer. Do not add a second cached label store.
5. Wire launch policy through both fresh starts and native resumes, then verify the complete visible result with a real CLI turn.

Claude label rules:

- For the summarized first release, select the first meaningful line of public prose.
- Strip the captured helper's markdown forms, collapse whitespace, and cap the result at 200 Unicode scalar values including a single `…`. Treat this as an explicit Artisan limit; do not assume the minified helper counts graphemes.
- Accept titles without terminal punctuation. Ignore lines that reduce to empty formatting markers.
- During streaming, reveal the current first meaningful line as it grows; later paragraphs do not replace it. A buffered authoritative correction may update it. Use existing UI update scheduling without new animation or timers.
- If no label exists, retain normal narration. End-of-turn rendering follows Artisan's existing settled status and duration behavior.

Do not claim that receipt time of the first public summary is model thinking start time. Retain existing turn-duration narration; exact per-thinking-block duration is outside this release unless the capture provides suitable timing and the domain already represents it.

Acceptance: a supported Claude turn shows a useful live line through the durable observation pipeline, survives replay, and leaves answer text, tools, usage, child attribution, Codex rendering, and older-CLI execution intact.

### 2.6 Packet C: hosted highlights experiment

Use Part 1's `--thinking-display highlights` finding directly in a separate fixture capture with the same execution context as Artisan. A paid subscription alone is not proof of Anthropic-hosted eligibility. Record success or rejection as an observed property of that context.

Resolve these questions from real frames before implementing highlight projection:

- Does `summaries` arrive in block starts, incremental events, buffered assistant frames, or several of these?
- Does a title arrive while the phase is running or only after it finishes?
- Are successive values appended titles, revised titles, or complete array snapshots?
- Can the stream expose the CLI's rejection/downgrade explicitly, or is it visible only in diagnostics?
- How do empty thinking and signature frames behave after the internal retry?

Map the last non-empty server title to the current phase. Do not concatenate replacement snapshots as deltas. If the existing observation shape cannot represent the observed live replacement semantics, document and implement the smallest shared contract extension, including codecs and replay tests, before enabling live highlights. Do not misuse a completion event to mark an active phase settled. If titles arrive only on completion, record that limitation rather than promising live Claude-app parity.

Acceptance: fixtures prove eligible delivery, ordering, and downgrade behavior. Until then, highlights remains an internal experiment and the shipped default remains summarized.

### 2.7 Packet D: highlight selection and fallback

Enable automatic highlights only for a context with positive evidence from Packet C. Keep requested mode, effective launch mode, and observed output distinct in diagnostics. Do not log summary contents or opaque signatures.

| Condition | This process | Next launch |
| --- | --- | --- |
| No display capability | Existing CLI behavior and generic status | Omit flag |
| Capability known; highlight eligibility unknown | Request summarized | Summarized |
| Experimentally verified eligible context | Request highlights and display recognized titles | Highlights while evidence remains valid |
| Explicit observed highlight rejection | Let the CLI perform its internal omitted retry; generic status if no public text | Summarized |
| Completed highlighted turn with observed thinking blocks but no usable titles or rejection signal | Generic status; record unavailable output, not proven ineligibility | Summarized as a UX fallback |
| Cancelled/failed/no-thinking turn | Preserve normal outcome and narration | Do not infer capability loss |

The owner must retain fallback state across finite CLI processes in the existing session/runtime ownership layer. Key it by provider session, profile/provider, model, and CLI version; clear it on relevant changes or runtime restart. Do not add durable database state for a transient capability result. If that ownership layer cannot retain the decision reliably, keep automatic highlights disabled until it can.

There is no same-turn recovery promise. Never restart or replay a user turn just to obtain a label. The fallback changes arguments on the next ordinary launch or native resume; it does not send `set_max_thinking_tokens` or change thinking budgets. Consequently the CLI's refusal of control overrides after explicit launch flags does not affect this design.

A control-request implementation is needed only for a future feature that changes display during a running turn. Keep the desktop on-demand findings in Part 1 as the starting point for that work.

### 2.8 Verification and completion criteria

Add focused tests to the repository's existing test registration structure. New fixture helpers must be registered through its normal mechanism.

| Area | Required cases |
| --- | --- |
| Launch/probe | Supported summarized flag, older CLI unchanged, fresh start and resume, continuation floor unchanged |
| Parser/adapter | Mixed text/thinking/tools, multiple blocks, streamed plus buffered text, authoritative correction, empty thinking, usage emitted once |
| Identity/replay | Stable item IDs, duplicate-frame handling, run and turn isolation, child frames, native resume |
| Bounds/outcomes | Unicode fragment boundaries, oversized accumulation, malformed summaries, signatures excluded, cancellation and EOF cleanup |
| Presentation | Unpunctuated title, blank/markdown-only lines, headings/lists/links/emphasis/fences, Unicode truncation, streaming first line, unchanged Codex policy |
| Integration | Observation persistence and replay produce the same line, status visibility agrees with copy, settled lifecycle stays intact |
| Highlight follow-up | Append versus replacement semantics, explicit rejection, empty-output fallback, no downgrade from cancellation or no-thinking turns, reset on context change |

Run focused suites for each packet, then the repository gate before considering implementation complete:

```sh
cargo test -p artisan-backend engine_owner_claude
cargo test -p artisan-native-engine --test claude_probe --test claude_discovery
cargo test -p artisan-ui --test inline_code_text
cargo test -p artisan-domain -p artisan-protocol -p artisan-frontend
python3 scripts/check.py
```

Confirm package names and test filters against the checkout when implementing. Record any pre-existing gate failures separately; changed files must meet their own allowances. Do not raise the file-size ratchet.

For Packet B, record one real summarized fresh turn and one resumed turn, including when the first label appears. Record launch compatibility with an older supported binary. For highlights, capture both an eligible result and fallback evidence before release. Add repeatable capture steps to `docs/runbooks/native-dev.md` and the final internal-field decision under `docs/decisions/`.

### 2.9 Recommended implementation order

Implement A and B as the first complete release. B includes label formatting, replay coverage, status integration, and native verification. Investigate C independently of that release. Implement D only once C proves it useful and the fallback state has an explicit owner.

## Appendix A - Evidence artifacts (session-local, not checked in)

| Artifact | Path | Notes |
| --- | --- | --- |
| Extracted desktop app.asar | `/tmp/opencode/claude-app` (~42 MB) | `.vite/build/`, `.vite/renderer/`, `package.json` |
| Copied app.asar | `/tmp/opencode/claude-app.asar` (~40 MB) | source: MSIX install |
| claude.ai web chunks | `/tmp/opencode/claude-web` (66 files, ~19 MB) | includes `shared-11`, `shared-12`, `shared-19`, `shared-msg-*` |
| claude.exe strings | `/tmp/opencode/claude-cli-strings.txt` (~52 MB, 513,929 lines) | from 242 MB binary in the npm `@anthropic-ai/claude-code` install |

Key code sites in the artifacts:

| Finding | Artifact / identifier |
| --- | --- |
| Short label derivation | `claude-web/shared-12-4P3rNZ7K.js` (`Vp`, `Hp`, `Dg`, `th`, block builder) |
| First-line and truncation helpers | `claude-web/shared-11-CETOUHfJ.js` (`oO`, `sO`, `cO`, `lO`, `uO`, `dO`) |
| Duration strings | `shared-11` (`Thought for ...`), `shared-19-Yi-rkk6b.js` (`{seconds}s`) |
| On-demand flip | `claude-app/.vite/build/index.chunk-7Eg3g-H9.js` (`DD`, `ED`, `vk`, `gk`) |
| Control schema and bridge | `claude-app/.vite/build/index.chunk-DAdPDze6.js`, `index.chunk-CHNweogn.js` |
| CLI display enum and help | `claude-cli-strings.txt`: `QNe`, `--thinking-display`, `Smo` |
| Highlights description and control error | `claude-cli-strings.txt` (control description; `set_max_thinking_tokens` error) |
| Rejection regex and latch | `claude-cli-strings.txt`: `pbe`, `oce`, `N1r`, `Q$t`, `markThinkingHighlightsRefused` |
| Beta header | `thinking-display-updates-2026-08-18` (`CLAUDE_CODE_THINKING_DISPLAY_UPDATES`) |

## Appendix B - Public references

- Anthropic extended thinking announcement: <https://www.anthropic.com/research/visible-extended-thinking>
- Thinking overview (display, summaries, billing): <https://platform.claude.com/docs/en/build-with-claude/thinking>
- Extended thinking (manual mode, deprecated budget): <https://platform.claude.com/docs/en/build-with-claude/extended-thinking>
- Third-party observation that Claude Code's visible thinking is a summary, not raw CoT: <https://patrickmccanna.net/the-text-in-claude-codes-extended-thinking-output-is-not-authentic>
