# Claude thinking display and internal highlight fields

Status: accepted on 2026-09-25 for the public-summary release; revised
2026-09-27 to stop requesting hosted `highlights` by default after the server
was proven to refuse it outside Anthropic-hosted execution contexts, keeping
the highlights machinery behind the evidence gate (Packets C and D of
[the thinking-highlights plan](../plans/claude-thinking-highlights.md) are
partially implemented); extended 2026-09-26 with the collapsed-chip
presentation (below).

## Decision

Artisan's backend display policy requests `--thinking-display summarized` from
Claude Code releases at or above the captured known-good floor `2.1.282`
(`CLAUDE_THINKING_DISPLAY_VERSION`). `summarized` carries public summary prose
through the existing reasoning observations, and presentation reduces it to the
one-line thinking label.

`highlights` is **not** requested by default. It is internal, undocumented, and
the server accepts it only for execution contexts Anthropic hosts, so a local
subscription launch loses the entire thinking trace. The decoding path stays
implemented and tested so a future eligible context can use it: a recognized
server title (`summaries[].summary`, last non-empty wins) supplies the label and
the buffered assistant frame settles the stretch with the newest title. Enabling
it is a one-line change in `ClaudeThinkingDisplay::for_support` once an
eligible context is captured.

The owner also keeps the observed-refusal memory. The CLI performs no
client-side eligibility check: on the API's HTTP 400 rejection it latches and
silently retries with `omitted`, which looks like thinking blocks with no text
or title. The owner records that refusal per context — profile, explicit target
model, CLI version — in owner-lifetime memory and requests `summarized` for
that context on subsequent launches. Nothing is persisted; a runtime restart
re-probes.

## Evidence

- The 2026-09-25 captures (`tests/fixtures/claude/manifest.json`) prove
  `summarized` is accepted by 2.1.282 and that `thinking_delta` then carries
  public summary prose, with a buffered `assistant` frame repeating each
  block's authoritative text before its `content_block_stop`.
- The 2026-09-27 capture (`highlights-refused.jsonl`) proves the same release
  accepts the flag but this first-party subscription context is ineligible: the
  API answers 400 `thinking.adaptive.display: Input should be 'summarized',
  'omitted'`, the CLI logs `[thinking] server rejected thinking.display
  highlights; asking for omitted for the rest of this process and retrying.`,
  then streams empty thinking blocks with no `summaries` and exits 0. The
  rejection is invisible on stdout, stderr, and the result frame; only the
  CLI's own debug log (Windows-side `~/.claude/debug/<session>.txt`) records
  it.
- `constructed-highlight-titles.jsonl` encodes the claude.ai client contract
  for decode tests. No eligible capture exists yet, so title arrival timing
  while streaming remains unverified; a title recognized at block start
  projects immediately, and anything later rides the authoritative completion.
- The refusal mechanism is conclusive from the pinned 2.1.282 binary, and
  there is no client-constructible request that avoids it:

  ```js
  // request build — the only gate is the process latch, not eligibility
  Zg = r.display === "highlights" && Cjt() ? "omitted" : r.display
  function Cjt(){ return n().host.requestLatches.thinkingHighlightsRefused() }

  // retry reducer — detect the 400, latch, retry as omitted
  function gwe(e){ return e instanceof Ot && e.status === 400 &&
    /thinking\.(adaptive|enabled)\.display: Input should be /.test(e.message) }
  if (U1e === "highlights" && gwe(qn)) return Zqr(), /* ... */ "retry:thinking-display-highlights"
  function Zqr(){ n().host.requestLatches.markThinkingHighlightsRefused() }
  ```

  `highlights` carries **no** beta header (the only thinking-display beta is
  `thinking-display-updates-2026-08-18`, for `updates`), so the client cannot
  opt in with a flag. Eligibility is decided server-side, and the sole
  distinguishing request header the CLI can send is
  `x-claude-remote-session-id`, which it populates solely from
  `process.env.CLAUDE_CODE_REMOTE_SESSION_ID` — set only when Anthropic hosts
  the session. A local subscription launch never sends it, so the server's
  enum stays `('summarized', 'omitted')`. This is why `summarized` is the
  default and the highlights path is evidence-gated rather than probed.

## Consequences

- First turn per unproven context: highlights is attempted; on refusal that
  turn loses thinking text to the CLI's omitted retry, then the context falls
  back to `summarized`. Capability loss is never inferred from cancelled,
  failed, or no-thinking turns.
- The capability is carried on `VerifiedClaudeLaunch::thinking_display()`
  (`DisplayControl` vs `Unsupported`), separate from
  `CLAUDE_MINIMUM_CLI_VERSION` and continuation gating. It distinguishes flag
  support from highlights eligibility, which no version implies.
- The display is a backend-local launch policy (`ClaudeThinkingDisplay`), not
  a persisted `ClaudeSelection` field. An explicit user override would need
  its own precedence and persistence decision.
- Titles and prose flow through the existing `ReasoningSummaryDeltaObservation`
  and `ReasoningSummaryCompletedObservation`; storage, wire codecs, and
  frontend pairing are unchanged. The Codex `summary_line` policy is unchanged;
  Claude turns use `claude_label_line` through `SummaryLinePolicy`.
- `summarized` bills the full thinking tokens, exactly as the flagless default
  already does; only the returned text changes. `highlights` replaces prose
  with titles under the same billing.
- A newer CLI, an eligible context, or a transport-shape change must be
  recaptured with `scripts/claude_thinking_capture.py` before the floor moves
  or the fallback is removed.

## Collapsed thinking chips (2026-09-26) — withdrawn 2026-09-28

Withdrawn on Sander's feedback ("thinking stuff bleeding into the header"): the
work-group header now carries only the turn's own outcome (`Worked for …`,
`Thought for …`, `Interrupted`, `Failed`, `Cancelled`) or the live verb with its
elapsed time, and thinking text stays on the reasoning rows and the live status
row. The record below describes the withdrawn design.


A Claude thinking stretch now leaves the app-style one-line trace: the reduced
label titles the collapsed work-group header as `label · duration`
(`Checking pairwise sums of the values · 13s`), the way the Claude app's
collapsed thinking header does.

- The label is the first meaningful line of the reasoning text trimmed to its
  first clause (`claude_first_clause`). When an eligible context returns
  `summaries`, that server-authored title is the reasoning text and the local
  reduction only trims it; otherwise the opening phrase of the public summary
  prose is cut at the first clause separator.
- The live status row no longer repeats the label under `Thinking for Xs`;
  the chip is the line's only surface when an owning work group carries it.
  Every other engine keeps the previous behavior: a summary row beside the
  elapsed header, and the Codex sentence policy unchanged.
- A settled thinking group keeps its newest thinking body
  (`WorkGroupBlock.reasoning_summary` is no longer live-only) so the chip
  survives settlement like the app's chips. `TurnStatusBlock.reasoning_summary`
  stays live-only: the status row is a live surface.
- Work sessions are unchanged: `Working`/`WorkedFor` headers keep their work
  verbs, and a thinking label beside them stays a status row.
