# Engine-usage frontend adapter contract (frozen)

This document freezes the request/response shapes the root UI glue implements
against the Forge `ReadAccountUsage` query. It is the only frontend-facing
surface of the engine-usage packet: no frontend product code changes in this
packet, and the adapter must not invent fields, infer quota surfaces, or
substitute token run-usage for account reads.

Wire path: `Request.readAccountUsage @25` /
`Response.accountUsage @26` in `modules/protocol/schema/artisan.capnp`,
carried by the existing QUIC request path with the triggering
`Envelope.messageId` echoed as `Response.requestId`. Queries carry no nested
request id; correlation is the outer frame identity only.

## Query JSON

```json
{
  "engine_id": "codex",
  "force": true
}
```

| Field       | Type             | Required | Bounds                                                     |
| ----------- | ---------------- | -------- | ---------------------------------------------------------- |
| `engine_id` | string           | no       | Shared identifier rule: non-empty, no whitespace/control, ≤128 UTF-8 bytes. Absent means every registered engine. Unknown ids do **not** fail the query (see below). |
| `force`     | boolean          | no       | Defaults to `false`. `true` bypasses the 60-second backend freshness window. |

Recommended client behavior: one query with absent `engine_id` for the first
paint, then per-engine narrowed queries (`engine_id` set) to repaint single
rows. `force: true` only on explicit user refresh. A narrowed single-engine
snapshot carries exactly that engine's observation time in `fetched_at`;
an aggregate snapshot carries the latest observation time across its
reports, so clients that need exact per-engine freshness issue one narrowed
query per engine.

## Snapshot JSON

```json
{
  "engines": [
    {
      "engine_id": "codex",
      "display_name": "Codex",
      "authentication": "authenticated",
      "auth_reason": null,
      "account_email": "owner@example.test",
      "quota_surface": "supported",
      "failure": null,
      "windows": [
        {
          "id": "codex:primary",
          "kind": "session",
          "label": null,
          "percent_used": 42.5,
          "resets_at": "2026-09-09T12:00:00Z",
          "window_minutes": 300
        }
      ]
    }
  ],
  "fetched_at": "2026-09-09T12:00:00Z"
}
```

| Field            | Type             | Required | Notes |
| ---------------- | ---------------- | -------- | ----- |
| `engines`        | array            | yes      | 0–16 reports in backend roster order: `codex`, `claude`, `cursor`, `grok`, `hermes`, `opencode2`, or exactly one report when `engine_id` narrows (even for unknown ids). |
| `fetched_at`     | ISO-8601 string  | yes      | Shared fetch instant for every report in the snapshot. |
| `engine_id`      | string           | yes      | Stable id (`codex`, `claude`, `cursor`, `grok`, `hermes`, `opencode2`, or the echoed unknown id). |
| `display_name`   | string           | yes      | `Codex`, `Claude`, `Cursor`, `Grok Build`, `Hermes`, `OpenCode`. |
| `authentication` | enum             | yes      | `authenticated` \| `unauthenticated` \| `unknown`. |
| `auth_reason`    | string \| null   | no       | Artisan-owned reason (≤1024 bytes), e.g. `Sign in to Cursor from Settings.` Never a provider payload. |
| `account_email`  | string \| null   | no       | Provider account email when disclosed (Codex ChatGPT accounts). |
| `quota_surface`  | enum \| null     | no       | `supported` \| `unknown` \| `unsupported`. **Never infer this from an empty `windows` list.** |
| `failure`        | string \| null   | no       | Artisan-owned failure reason when the read failed. Never a provider payload. |
| `windows`        | array            | yes      | 0–64 quota windows. |

Window fields:

| Field            | Type             | Required | Notes |
| ---------------- | ---------------- | -------- | ----- |
| `id`             | string           | yes      | Provider bucket id (`five_hour`, `seven_day`, `codex:primary`, `cursor:cursor-models`, …). Stable per provider; use as row key. |
| `kind`           | enum             | yes      | `session` \| `weekly` \| `monthly` \| `unknown`. Billing cadence, not a display label. |
| `label`          | string \| null   | no       | Provider human bucket name (`Fable`, `Cursor models`, …). Render verbatim when present; never invent one when absent. |
| `percent_used`   | number           | yes      | Always finite, always clamped to `0–100`. Render as-is (one decimal max); never recompute from other fields. |
| `resets_at`      | ISO-8601 \| null | no       | Reset instant when the provider disclosed one the backend could resolve. Absent means unknown, never "no limit". |
| `window_minutes` | integer \| null  | no       | Cadence in minutes (300, 10080, …) when disclosed. Absent means unknown. |

## Per-engine row rules

- Render one row per `engines` entry in array order.
- Row header: `display_name` plus `account_email` when present.
- One meter per `windows` entry: `label` (fallback: `id`), `percent_used`
  bar, and `resets_at`/`window_minutes` caption when present.
- `kind` selects the meter caption (`session` → "Session", `weekly` →
  "Weekly", `monthly` → "Monthly", `unknown` → provider `id`), never a
  hardcoded provider assumption.

## Failure rendering rules

1. `failure != null` → render the row in its failure state with the
   `failure` string verbatim and no meters. The string is Artisan-owned and
   safe to display.
2. `authentication == "unauthenticated"` → render the sign-in state with
   `auth_reason` (e.g. `Sign in to Cursor from Settings.`,
   `Cursor sign-in is no longer valid.`, `Codex account sign-in is
   required.`). Offer the provider's normal sign-in entry, never a token
   input: this surface performs no login, token write, or key creation.
3. `quota_surface == "unsupported"` (Grok Build, Hermes, OpenCode) → render
   the honest unsupported state with `failure`
   (`<Display> exposes no account-usage surface.`). Never hide the row and
   never show zeroed meters.
4. `quota_surface == "unknown"` → render a retryable "could not reach the
   provider" state with `failure`. The backend retries on the next query;
   `force: true` re-asks immediately.
5. Unknown `engine_id` narrowing answers one report echoing the id with
   `failure: "unknown engine id"`. Render it as an unknown-engine row, not a
   connection error.
6. Missing backend capability answers `protocolError` code
   `unsupportedFeature` (`account usage service is unavailable`,
   non-retryable). Render the whole surface unavailable, not per-row
   failures.
7. Never substitute token run-usage (per-turn/per-run token counts) for
   these account reads, and never render an empty `windows` list with
   `quota_surface == "supported"` as "no quota": it means the account has no
   configured spend limit.

## Freshness contract

- Reports cache per engine for 180 seconds, matching the Electron service;
  repeated queries inside the window return identical bytes without
  contacting providers.
- `force: true` re-asks every selected provider even inside the window.
- A failed refresh preserves the last-good report with its original fetch
  time and marks the served copy with the refresh failure, so stale data is
  never stamped with the current clock. A `failure` string on a report that
  still carries windows means exactly this: render the last-good meters with
  a stale warning, not an empty failure state.
- `fetched_at` identifies the snapshot; rows from one snapshot are mutually
  consistent. Do not mix rows across snapshots when painting per-engine
  fan-out results.

## Known backend limitations (visible to UI copy)

- Claude reset instants resolve only for UTC-equivalent zones; other zones
  omit `resets_at` rather than guessing. Meters still show exact percents.
