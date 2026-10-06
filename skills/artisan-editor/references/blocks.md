# Rich block catalog

Every kind, its fields, and one working example. `(required)` marks the
fields a kind cannot draw without; everything else is optional. Every kind
also accepts the envelope fields `title` and `caption`.

Shared values:

- `tone`: `neutral` · `info` · `success` · `warning` · `danger` · `accent`
- `status`: `done` · `active` · `pending` · `failed` · `skipped` · `blocked`
  (also `pass`, `fail`, `skip`, `warn`)
- `ref`: `"path"`, `"path:42"`, or `"path:42-60"`, relative to the project

Contents: [Structure](#structure-and-narrative) ·
[Code](#code-and-repository) · [Data](#data-and-charts) ·
[Agent work](#agent-work) · [Media](#media-and-small-pieces)

---

## Structure and narrative

### header

Opens a walkthrough. Drawn without a card.
`title` (required), `eyebrow`, `summary`, `refs: [{ label, ref?, url? }]`.

```artisan-header
{
  "eyebrow": "api #5474 · verified against 328b508",
  "title": "Teach the CLI what dispatch accepts",
  "summary": "The CLI's broad depot-* classifier cannot prove CI support. This PR derives a policy from backend code.",
  "refs": [{ "label": "cli #622", "url": "https://github.com/acme/cli/pull/622" }, { "label": "runs-on-labels.ts", "ref": "src/runs-on-labels.ts" }]
}
```

### callout

A note set apart. `body` (required), `kind`: `note` · `tip` · `warning` ·
`risk` · `decision` · `assumption`, `collapsed: bool` (starts closed; click to
open). The `title` sits on the callout's label line.

```artisan-callout
{ "kind": "warning", "title": "Stale copy fails CI", "body": "ci recomputes the policy; --check fails if the committed copy is stale, without writing." }
```

### cards

Side-by-side cards. `cards: [{ title (required), eyebrow?, body?, code?,
footer?, tone?, details? }]` (required), `columns` (1–4). Cards with
`details` expand on click.

```artisan-cards
{
  "title": "Two consumers",
  "cards": [
    { "eyebrow": "live dispatch", "title": "Resolve, check, dispatch", "body": "Resolve a job's labels, call the shared check, dispatch or throw.", "footer": "size rejection still records architecture first" },
    { "eyebrow": "static generation", "title": "Iterate the mapping", "body": "Call the same check and emit sorted {label, supported} entries.", "footer": "schema 1 · 138 labels · 99 supported" }
  ]
}
```

### flow

Boxes joined by arrows, ranked by longest path so forks sit side by side.
`nodes: [{ id, title, body?, code?, ref? }]` (required),
`edges: [{ from, to, label? }]`, `direction`: `down` (default) · `right`.
Hovering a node highlights its neighbors.

```artisan-flow
{
  "title": "One source, two paths",
  "nodes": [
    { "id": "map", "title": "Default label → machine mapping", "code": "runs-on-labels.ts" },
    { "id": "check", "title": "Shared support check", "code": "ciDispatchMachineSupport(label, machine)" },
    { "id": "live", "title": "Live dispatch" },
    { "id": "static", "title": "Static generation" }
  ],
  "edges": [{ "from": "map", "to": "check" }, { "from": "check", "to": "live" }, { "from": "check", "to": "static" }]
}
```

### steps

A numbered process. `steps: [{ title (required), body?, status?, details? }]`
(required). Clicking a number marks the step done; steps with `details`
expand.

```artisan-steps
{
  "title": "How a backend change reaches users",
  "steps": [
    { "title": "Change labels, mappings, or support gates; regenerate the API json", "status": "done" },
    { "title": "CI recomputes it", "body": "--check fails if the committed copy is stale." },
    { "title": "After merge, sync resolves current main", "details": "It reads and validates the file at that exact sha." },
    { "title": "Review and merge the CLI update, then release" }
  ]
}
```

### before-after

Two states. `before`, `after`: `{ label?, body?, code?, language? }` (both
required). A toggle switches split and stacked.

```artisan-before-after
{
  "before": { "label": "Before", "code": "if (label.startsWith('depot-')) return true;" },
  "after": { "label": "After", "code": "return ciDispatchMachineSupport(label, machine);" }
}
```

### compare

Options against criteria. `options: [string]` (required),
`rows: [{ criterion, values: [string] }]` (required), `recommended` (one of
the options). Hovering a column highlights it.

```artisan-compare
{
  "options": ["Render HTML", "Native blocks"],
  "recommended": "Native blocks",
  "rows": [
    { "criterion": "Matches the app", "values": ["Needs CSS token bridge", "Uses theme tokens"] },
    { "criterion": "Security", "values": ["Sandbox required", "No scripts"] },
    { "criterion": "Coverage", "values": ["Anything", "The catalog"] }
  ]
}
```

### decision

A decision record. `choice` (required), `status`: `proposed` · `accepted` ·
`rejected` · `superseded`, `context`, `options: [{ name, pros?: [string],
cons?: [string] }]`, `consequences: [string]`.

```artisan-decision
{
  "status": "accepted",
  "context": "Agents want rich output; the Editor is native GPUI.",
  "options": [
    { "name": "Embedded web view", "pros": ["Unlimited"], "cons": ["Second visual system", "Sandboxing"] },
    { "name": "Declarative blocks", "pros": ["Native", "Themed"], "cons": ["Catalog-bound"] }
  ],
  "choice": "Declarative blocks",
  "consequences": ["Each new kind is engineering work", "Replies stay copyable as Markdown"]
}
```

### timeline

Events in order. `events: [{ at (required), title (required), body?, tone? }]`
(required). Hovering an event shows its body.

```artisan-timeline
{
  "events": [
    { "at": "14:02", "title": "Deploy 4812 starts" },
    { "at": "14:09", "title": "Error rate crosses 2%", "tone": "danger", "body": "Checkout requests time out against the new pool." },
    { "at": "14:15", "title": "Rolled back", "tone": "success" }
  ]
}
```

### glossary

Terms as chips; hovering one shows its definition.
`terms: [{ term, definition }]` (required).

```artisan-glossary
{ "terms": [{ "term": "Dispatch", "definition": "Sending a CI job to a runner machine." }, { "term": "Label", "definition": "The runs-on value a job asks for." }] }
```

### tabs

One view at a time. `tabs: [{ label, body?, block? }]` (required). `block` is
a nested rich block whose `type` names its kind.

```artisan-tabs
{
  "tabs": [
    { "label": "Summary", "body": "Turns grew 3x after the launch." },
    { "label": "Numbers", "block": { "type": "stats", "tiles": [{ "label": "turns", "value": "24.0M" }, { "label": "installs", "value": "85.0k" }] } }
  ]
}
```

---

## Code and repository

### code-ref

A pointer into the code. `ref` (required), `excerpt`, `language`, `note`.
The excerpt is numbered from the ref's first line; the path opens the file.

```artisan-code-ref
{ "ref": "src/dispatch/support.ts:12-16", "language": "ts", "excerpt": "export function ciDispatchMachineSupport(label, machine) {\n  if (machine.os !== 'ubuntu') return false;\n  return machine.cpus < 96;\n}", "note": "The only gate both paths share." }
```

### annotated-code

Code with margin notes. `code` (required), `language`, `path`, `start_line`
(default 1), `notes: [{ lines: 4 | "3-5", text }]`. Hovering a note
highlights its lines.

```artisan-annotated-code
{
  "path": "src/pool.rs",
  "start_line": 40,
  "code": "let pool = Pool::new(cfg)?;\nlet conn = pool.get().await?;\nconn.execute(sql).await?;",
  "notes": [{ "lines": 41, "text": "Waits forever when the pool is exhausted." }, { "lines": "40-41", "text": "The pool is rebuilt per request." }]
}
```

### diff

A focused change. `path` (required), `summary`, and `patch` (unified diff
text) or `hunks: [{ header?, lines: [string] }]` with lines starting `+`,
`-`, or a space.

```artisan-diff
{
  "path": "src/pool.rs",
  "summary": "Share one pool and bound the wait.",
  "hunks": [{ "header": "@@ -40,3 +40,3 @@", "lines": ["-let pool = Pool::new(cfg)?;", "+let pool = state.pool.clone();", "-let conn = pool.get().await?;", "+let conn = timeout(WAIT, pool.get()).await??;", " conn.execute(sql).await?;"] }]
}
```

### file-tree

Files touched. `entries: [{ path (required), change?: added · modified ·
deleted · renamed, note? }]` (required). Directories collapse; files open.

```artisan-file-tree
{ "entries": [{ "path": "src/pool.rs", "change": "modified" }, { "path": "src/state.rs", "change": "modified", "note": "Holds the shared pool" }, { "path": "tests/pool_timeout.rs", "change": "added" }] }
```

### symbol-graph

Calls and dependencies around one symbol. `nodes: [{ id, label?, kind? }]`
(required), `edges: [{ from, to, label? }]`, `focus` (a node id).

```artisan-symbol-graph
{
  "focus": "check",
  "nodes": [{ "id": "dispatch", "label": "dispatchJob" }, { "id": "generate", "label": "generatePolicy" }, { "id": "check", "label": "ciDispatchMachineSupport" }],
  "edges": [{ "from": "dispatch", "to": "check" }, { "from": "generate", "to": "check" }]
}
```

### architecture

Modules and boundaries. `groups: [{ id, label, nodes: [{ id, label? }] }]`,
`nodes` (ungrouped), `edges: [{ from, to, label? }]`.

```artisan-architecture
{
  "groups": [
    { "id": "backend", "label": "Backend", "nodes": [{ "id": "api", "label": "API" }, { "id": "policy", "label": "Policy" }] },
    { "id": "cli", "label": "CLI", "nodes": [{ "id": "sync", "label": "Sync" }] }
  ],
  "edges": [{ "from": "api", "to": "policy" }, { "from": "sync", "to": "api", "label": "reads json" }]
}
```

### sequence

Messages between actors. `actors: [string]` (required), `messages: [{ from,
to, text, kind?: sync · async · reply }]` (required).

```artisan-sequence
{
  "actors": ["CLI", "API", "Runner"],
  "messages": [
    { "from": "CLI", "to": "API", "text": "dispatch(job)" },
    { "from": "API", "to": "Runner", "text": "start", "kind": "async" },
    { "from": "API", "to": "CLI", "text": "202 Accepted", "kind": "reply" }
  ]
}
```

### state-machine

States and transitions. `initial` (required), `states: [{ id, label? }]`,
`final: [id]`, `transitions: [{ from, to, event? }]`.

```artisan-state-machine
{
  "initial": "queued",
  "states": [{ "id": "queued" }, { "id": "running" }, { "id": "done" }, { "id": "failed" }],
  "final": ["done", "failed"],
  "transitions": [{ "from": "queued", "to": "running", "event": "claim" }, { "from": "running", "to": "done", "event": "exit 0" }, { "from": "running", "to": "failed", "event": "exit ≠ 0" }]
}
```

### schema

Tables or types and their relations. `entities: [{ name, fields: [{ name,
type, key?: pk · fk, note? }] }]` (required), `relations: [{ from, to,
cardinality?, label? }]`.

```artisan-schema
{
  "entities": [
    { "name": "thread", "fields": [{ "name": "id", "type": "uuid", "key": "pk" }, { "name": "title", "type": "text" }] },
    { "name": "turn", "fields": [{ "name": "id", "type": "uuid", "key": "pk" }, { "name": "thread_id", "type": "uuid", "key": "fk" }] }
  ],
  "relations": [{ "from": "thread", "to": "turn", "cardinality": "one-to-many" }]
}
```

### api

A surface listing. `entries: [{ signature (required), method?, summary?,
params?: [{ name, type?, required?, doc? }], returns? }]` (required).
Entries expand; signatures copy on click.

```artisan-api
{ "entries": [{ "method": "POST", "signature": "/v1/dispatch", "summary": "Queue a job.", "params": [{ "name": "label", "type": "string", "required": true }], "returns": "202 with a job id" }] }
```

### test-matrix

Cases and results; status chips filter. Either `cases: [{ name, status,
detail? }]`, or `columns: [string]` with `rows: [{ name, results: [status] }]`.

```artisan-test-matrix
{ "columns": ["linux", "macos", "windows"], "rows": [{ "name": "dispatch_accepts_ubuntu", "results": ["pass", "pass", "pass"] }, { "name": "rejects_large_machines", "results": ["pass", "fail", "skip"] }] }
```

### command

A command to run. `command` (required), `cwd`, `description`, `output`,
`exit_code`. The command copies on click; long output collapses.

```artisan-command
{ "command": "cargo test -p artisan-ui rich", "description": "Run the rich block tests.", "output": "test result: ok. 42 passed", "exit_code": 0 }
```

---

## Data and charts

### stats

Headline figures. `tiles: [{ label (required), value (required), delta?,
trend?: up · down · flat, good?: up · down, sparkline?: [number], hint? }]`
(required). The delta is green when the trend is good.

```artisan-stats
{
  "tiles": [
    { "label": "installs sent a turn", "value": "85.0k" },
    { "label": "turns sent", "value": "24.0M", "delta": "+18%", "trend": "up", "good": "up", "sparkline": [3, 4, 4, 6, 9, 12] },
    { "label": "turns, median install", "value": "27" },
    { "label": "of turns from top 1%", "value": "18%", "hint": "Share of turns sent by the heaviest 1% of installs." }
  ]
}
```

### chart

Line, area, bar, scatter.
- `series: [{ name, values?: [number | null], points?: [[x, y]] }]` (required)
- `mark`: `line` (default) · `area` · `bar` · `scatter`; `stacked`, `normalize`
- `categories: [string]` — x values for `values`
- `x: { label?, unit? }`, `y: { label?, unit?, min?, max? }`
- `annotations: [{ x?, y?, x_end?, label }]` — rule at `x`, rule at `y`, band `x`…`x_end`
- `params: [{ id, kind: "threshold", label, min, max, step?, value }]` — a slider moving a horizontal rule, reporting the share of points at or above it

Hover reads every series at the nearest x; legend entries toggle; drag to
zoom; double-click resets.

```artisan-chart
{
  "title": "Opus 5.5 went from 0 to half of all turns in 3 days",
  "mark": "area",
  "stacked": true,
  "normalize": true,
  "categories": ["Sep 19", "Sep 20", "Sep 21", "Sep 22", "Sep 23", "Sep 24", "Sep 25"],
  "series": [
    { "name": "Opus 5.5", "values": [0, 0, 0, 12, 31, 44, 51] },
    { "name": "Opus 5", "values": [48, 47, 49, 40, 30, 22, 18] },
    { "name": "Other", "values": [52, 53, 51, 48, 39, 34, 31] }
  ],
  "y": { "unit": "%" },
  "annotations": [{ "x": "Sep 22", "label": "Opus 5.5 ships" }],
  "caption": "Share of sent turns per day, by model family."
}
```

### heatmap

A grid of values. `values: [[number | null]]` (required, rows follow `y`),
`x: [string]`, `y: [string]`, `unit`, `scale`: `sequential` · `diverging`.

```artisan-heatmap
{
  "title": "Peak is Thursday 3 PM, local time",
  "x": ["9a", "12p", "3p", "6p"],
  "y": ["Mon", "Tue", "Wed", "Thu", "Fri"],
  "values": [[40, 62, 70, 41], [44, 65, 74, 43], [45, 66, 77, 45], [47, 70, 88, 46], [42, 60, 66, 35]],
  "unit": "k turns"
}
```

### distribution

The shape of values. `series: [{ name, values: [number] }]` (required),
`mode`: `histogram` (default) · `box` · `cdf`, `bins`, `unit`, `threshold`.
Histogram and cdf modes get a threshold slider.

```artisan-distribution
{ "mode": "cdf", "unit": "s", "threshold": 60, "series": [{ "name": "Claude Code", "values": [12, 20, 33, 41, 45, 60, 75, 120, 300] }, { "name": "Codex", "values": [30, 55, 80, 93, 110, 150, 220, 400] }] }
```

### funnel

Conversion through steps. `steps: [{ label, value }]` (required).

```artisan-funnel
{ "steps": [{ "label": "Installed", "value": 120000 }, { "label": "Opened a thread", "value": 96000 }, { "label": "Sent a turn", "value": 85000 }, { "label": "Sent 10 turns", "value": 41000 }] }
```

### sankey

Flows between categories. `links: [{ from, to, value }]` (required),
`nodes: [{ id, label? }]` (derived from links when omitted).

```artisan-sankey
{ "links": [{ "from": "Turns", "to": "Claude", "value": 60 }, { "from": "Turns", "to": "Codex", "value": 40 }, { "from": "Claude", "to": "Completed", "value": 55 }, { "from": "Claude", "to": "Failed", "value": 5 }, { "from": "Codex", "to": "Completed", "value": 36 }, { "from": "Codex", "to": "Failed", "value": 4 }] }
```

### table

Rows of data. `columns: [{ key, label?, align?: left · right, kind?: text ·
number }]` (required), `rows: [object]` (required), `page_size` (default 20).
Headers sort; pages step.

```artisan-table
{
  "columns": [{ "key": "model", "label": "Model" }, { "key": "turns", "label": "Turns", "kind": "number" }, { "key": "share", "label": "Share", "kind": "number" }],
  "rows": [{ "model": "Opus 5.5", "turns": 12200000, "share": 51 }, { "model": "GPT-6.x", "turns": 6700000, "share": 28 }, { "model": "Fable 5.x", "turns": 960000, "share": 4 }]
}
```

### perf

Before and after. `unit` (required), `rows: [{ name, before, after }]`
(required), `lower_is_better` (default true). A toggle shows percent change.

```artisan-perf
{ "unit": "ms", "rows": [{ "name": "first paint", "before": 182, "after": 121 }, { "name": "scroll frame p99", "before": 14.2, "after": 9.8 }] }
```

---

## Agent work

### checklist

Acceptance criteria that tick locally. `items: [{ text, done?, note? }]`
(required).

```artisan-checklist
{ "items": [{ "text": "Policy derives from backend code", "done": true }, { "text": "CI fails on a stale copy", "done": true }, { "text": "CLI release notes updated" }] }
```

### progress

A status snapshot; re-emit to update. `items: [{ label, status, detail?,
progress?: 0..1 }]` (required).

```artisan-progress
{ "items": [{ "label": "pull request #5474", "status": "active", "detail": "2 checks running", "progress": 0.6 }, { "label": "pull request #622", "status": "pending" }] }
```

### verification

What was checked. `checks: [{ name, status, evidence? }]` (required),
`subject`, `commit`. Checks with evidence expand.

```artisan-verification
{ "subject": "api #5474", "commit": "328b508", "checks": [{ "name": "cargo test", "status": "pass", "evidence": "412 passed, 0 failed" }, { "name": "policy json is current", "status": "pass" }, { "name": "manual dispatch on macos", "status": "skip", "evidence": "No macOS runner available." }] }
```

### risks

Risks and mitigations, sorted by severity. `risks: [{ title, severity: low ·
medium · high · critical, likelihood?: low · medium · high, mitigation? }]`
(required).

```artisan-risks
{ "risks": [{ "title": "Old CLIs reject new labels", "severity": "high", "likelihood": "medium", "mitigation": "Ship the CLI update before the backend flag." }, { "title": "Policy json drifts", "severity": "medium", "likelihood": "low", "mitigation": "CI --check." }] }
```

### choice

A question the reader answers; picking sends the option's label as their
reply. Put it last. `question` (required), `options: [{ label (required),
description?, pros?: [string], cons?: [string], recommended?: bool }]`
(required).

```artisan-choice
{
  "question": "How should agents render rich output?",
  "options": [
    { "label": "Native blocks", "description": "Declarative JSON drawn natively.", "pros": ["Themed", "Safe"], "recommended": true },
    { "label": "Embedded HTML", "description": "A sandboxed web view per artifact.", "cons": ["Second visual system"] }
  ]
}
```

### log

A log excerpt. `text` or `lines: [string]` (one required), `highlight:
[line number]` (1-based). Level chips filter; long logs collapse.

```artisan-log
{ "lines": ["INFO pool: created 8 connections", "WARN pool: wait exceeded 5s", "ERROR checkout: timed out after 30s"], "highlight": [3] }
```

---

## Media and small pieces

### image

An image with numbered callouts at fractional positions. `src` (required,
project path or URL), `alt`, `width`/`height` (aspect hints),
`callouts: [{ x: 0..1, y: 0..1, label?, text }]`.

```artisan-image
{ "src": "docs/assets/composer.png", "alt": "The composer", "callouts": [{ "x": 0.12, "y": 0.2, "text": "Project tab" }, { "x": 0.5, "y": 0.2, "text": "Goal strip" }] }
```

### image-compare

Drag the divider between two images. `before`, `after`: `{ src, label? }`
(both required).

```artisan-image-compare
{ "before": { "src": "docs/assets/before.png", "label": "Before" }, "after": { "src": "docs/assets/after.png", "label": "After" } }
```

### swatches

Colors and tokens; clicking copies the value. `swatches: [{ name, value,
note? }]` (required), `value` as `#rrggbb`, `#rrggbbaa`, `oklch(l c h)`, or
`rgb(r, g, b)`.

```artisan-swatches
{ "swatches": [{ "name": "surface-800", "value": "oklch(0.274 0.006 286.033)" }, { "name": "accent", "value": "#7c5cff", "note": "Links and focus" }] }
```

### math

A formula. `tex` (required), `display` (default true): Greek letters,
operators, arrows, `^`, `_`, `\frac`, `\sqrt`, `\sum`, `\prod`, `\int`,
`\lim`, `\text`, `\mathbb`, `\mathbf`. Remember JSON escaping: `\\frac`.

```artisan-math
{ "tex": "p_{99} = \\frac{\\sum_{i} w_i x_i}{\\sum_{i} w_i}" }
```

### chips

A row of small labels, drawn without a card. `chips: [{ label, kind?: status
· kbd · file · commit · person · tag, tone? }]` (required).

```artisan-chips
{ "chips": [{ "label": "merged", "kind": "status", "tone": "success" }, { "label": "⌘K", "kind": "kbd" }, { "label": "src/pool.rs", "kind": "file" }, { "label": "328b508", "kind": "commit" }] }
```

### footnotes

Sources and citations; linked notes open. `notes: [{ text, title?, url?,
id? }]` (required).

```artisan-footnotes
{ "notes": [{ "text": "Telemetry export, Jul 6 – Oct 2.", "title": "Turn events", "url": "https://example.com/telemetry" }, { "text": "Local time is each user's own time zone." }] }
```
