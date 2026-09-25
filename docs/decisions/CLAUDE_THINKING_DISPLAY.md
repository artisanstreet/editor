# Claude thinking display and internal highlight fields

Status: accepted on 2026-09-25 for the public-summary release (Packets A and B
of [the thinking-highlights plan](../plans/claude-thinking-highlights.md)).

## Decision

Artisan requests `--thinking-display summarized` from Claude Code releases at
or above the captured known-good floor `2.1.282`
(`CLAUDE_THINKING_DISPLAY_VERSION`) and derives the one-line thinking label in
presentation code from the first meaningful line of the public summary prose.

Artisan does not request `highlights` and does not read the `summaries` wire
field. Both are internal, undocumented, and accepted only for contexts
Anthropic hosts; the CLI silently downgrades a rejected `highlights` request to
`omitted`. Their use stays a separate, evidence-gated experiment (Packet C)
with its own fixtures and decision.

## Rationale

- `summarized` is the public contract. The 2026-09-25 captures
  (`tests/fixtures/claude/manifest.json`) prove the hidden flag is accepted by
  2.1.282 and that `thinking_delta` then carries public summary prose, with a
  buffered `assistant` frame repeating each block's authoritative text before
  its `content_block_stop`.
- Without the flag the same blocks stream empty text, so the flag is required
  for any label; below the floor Artisan omits it and keeps ordinary
  narration.
- Unknown display semantics are treated as unavailable: thinking text is
  projected only when the launch itself requested `summarized`. Signatures,
  redacted blocks, and unknown fields never become observation text.

## Consequences

- The capability is carried on `VerifiedClaudeLaunch::thinking_display()`,
  separate from `CLAUDE_MINIMUM_CLI_VERSION` and continuation gating. It
  distinguishes flag support from highlights eligibility, which no current
  capability implies.
- The display is a backend-local launch policy (`ClaudeThinkingDisplay`), not
  a persisted `ClaudeSelection` field. An explicit user override would need
  its own precedence and persistence decision.
- Summaries flow through the existing `ReasoningSummaryDeltaObservation` and
  `ReasoningSummaryCompletedObservation`; storage, wire codecs, and frontend
  pairing are unchanged. The Codex `summary_line` policy is unchanged; Claude
  turns use `claude_label_line` through `SummaryLinePolicy`.
- `summarized` bills the full thinking tokens, exactly as the flagless default
  already does; only the returned text changes.
- A newer CLI that changes the transport shape must be recaptured with
  `scripts/claude_thinking_capture.py` before the floor moves.
