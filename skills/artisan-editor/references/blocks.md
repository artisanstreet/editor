# Rich block catalog

Every kind, its fields, and one working example. `(required)` marks the
fields a kind cannot draw without; everything else is optional. Every kind
also accepts the envelope fields `title` (the finding, as a sentence),
`subtitle` (one line of scope and units under it), and `caption`.

Shared values:

- `tone`: `neutral` · `info` · `success` · `warning` · `danger` · `accent`
- `ref`: `"path"`, `"path:42"`, or `"path:42-60"`, relative to the project

Contents: [Structure](#structure) ·
[Code](#code-and-repository) · [Data](#data-and-charts) ·
[Media](#images-models-and-audio)

---

## Structure

### callout

A note set apart. `body` (required), `kind`: `note` · `tip` · `warning` ·
`risk` · `decision` · `assumption`, `collapsed: bool` (starts closed; click to
open). The `title` sits on the callout's label line.

```artisan-callout
{ "kind": "warning", "title": "Stale copy fails CI", "body": "ci recomputes the policy; --check fails if the committed copy is stale, without writing." }
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

## Code and repository

### code-ref

A pointer into the code. `ref` (required), `excerpt`, `language`, `note`.
The excerpt is numbered from the ref's first line; the path opens the file.

```artisan-code-ref
{ "ref": "src/dispatch/support.ts:12-16", "language": "ts", "excerpt": "export function ciDispatchMachineSupport(label, machine) {\n  if (machine.os !== 'ubuntu') return false;\n  return machine.cpus < 96;\n}", "note": "The only gate both paths share." }
```

### annotated-code

Code with numbered notes under it. `code` (required), `language`, `path`,
`start_line` (default 1), `notes: [{ lines: 4 | "3-5", text }]`. Hovering a
note highlights its lines.

```artisan-annotated-code
{
  "path": "src/pool.rs",
  "start_line": 40,
  "code": "let pool = Pool::new(cfg)?;\nlet conn = pool.get().await?;\nconn.execute(sql).await?;",
  "notes": [{ "lines": 41, "text": "Waits forever when the pool is exhausted." }, { "lines": "40-41", "text": "The pool is rebuilt per request." }]
}
```

### file-tree

Files touched. `entries: [{ path (required), change?: added · modified ·
deleted · renamed, note? }]` (required). Directories collapse; files open.

```artisan-file-tree
{ "entries": [{ "path": "src/pool.rs", "change": "modified" }, { "path": "src/state.rs", "change": "modified", "note": "Holds the shared pool" }, { "path": "tests/pool_timeout.rs", "change": "added" }] }
```

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

### concentration

How lopsided a total is across items: ranks items largest first and draws
the cumulative share of the total against the share of items, beside the
even-spread diagonal. Answers "the top 10% of installs send 65% of turns".
`values: [number]` (required, one per item), `items` (plural noun),
`total` (plural noun), `highlight` (share of items to call out, default 0.1).

```artisan-concentration
{ "title": "The top 10% of installs send 65% of turns", "subtitle": "Turns per install, last 28 days.", "items": "installs", "total": "turns", "highlight": 0.1, "values": [900, 420, 310, 120, 80, 60, 45, 30, 22, 18, 15, 12, 10, 9, 7, 6, 5, 4, 3, 2] }
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

### hierarchy

What a total is made of, nested: disk use, bundle size, time in a profile.
The spec is the root node, `{ name?, value?, children?: [node] }`, nested to
any depth; a node's size is its `value` or its children's sum, whichever is
larger (the difference is its own share, such as self time). `layout`:
`treemap` (default), `icicle`, or `flame` (an upside-down icicle, for
profiles). `unit` labels sizes; `depth` sets how many levels show below the
focus (treemap 2, others 6). The reader clicks a group to zoom in.

```artisan-hierarchy
{ "title": "Charts are a third of the renderer", "subtitle": "Lines of Rust under modules/ui/src/markdown_renderer.", "name": "markdown_renderer", "unit": "lines", "children": [{ "name": "rich", "children": [{ "name": "charts", "children": [{ "name": "chart", "value": 1492 }, { "name": "distribution", "value": 852 }, { "name": "sankey", "value": 765 }, { "name": "frame.rs", "value": 440 }, { "name": "heatmap.rs", "value": 387 }] }, { "name": "media", "children": [{ "name": "color.rs", "value": 720 }, { "name": "image_compare.rs", "value": 560 }] }, { "name": "code", "value": 1100 }, { "name": "narrative", "value": 600 }] }, { "name": "prepared.rs", "value": 900 }, { "name": "table.rs", "value": 520 }, { "name": "inline.rs", "value": 410 }] }
```

```artisan-hierarchy
{ "title": "Most of a turn is waiting on the model", "subtitle": "Wall time of one turn, ms.", "layout": "flame", "name": "turn", "unit": "ms", "value": 4200, "children": [{ "name": "stream", "value": 3600, "children": [{ "name": "first token", "value": 1400 }, { "name": "tokens", "value": 2100 }] }, { "name": "tools", "value": 450, "children": [{ "name": "read", "value": 180 }, { "name": "grep", "value": 220 }] }, { "name": "render", "value": 120 }] }
```

### spans

Intervals on a shared time axis. `spans: [{ label, start, end, lane?,
group? }]` (required); `start`/`end` are numbers in `unit` or ISO dates
(`"2026-10-07"`), not mixed. Without `lane`s every span gets its own row
(a request waterfall); with lanes, spans in a lane share its row and
overlapping ones stack beneath (a Gantt chart, swimlanes, or a flame chart
of trace spans). Bars color by `group`, else lane. `markers: [{ at, label }]`
draw labelled rules (a deadline, "today", an incident).

```artisan-spans
{ "title": "TLS is the slowest step", "subtitle": "One cold request, ms.", "unit": "ms", "spans": [{ "label": "DNS", "start": 0, "end": 24 }, { "label": "Connect", "start": 24, "end": 61 }, { "label": "TLS", "start": 61, "end": 180 }, { "label": "Request", "start": 180, "end": 196 }, { "label": "Wait", "start": 196, "end": 302 }, { "label": "Download", "start": 302, "end": 340 }] }
```

```artisan-spans
{ "title": "Sync ships before the Marketplace UI", "markers": [{ "at": "2026-10-07", "label": "Today" }], "spans": [{ "label": "Store and links", "lane": "Backend", "start": "2026-09-28", "end": "2026-10-03", "group": "Done" }, { "label": "Engine sync", "lane": "Backend", "start": "2026-10-03", "end": "2026-10-10", "group": "In progress" }, { "label": "Marketplace", "lane": "Frontend", "start": "2026-10-06", "end": "2026-10-16", "group": "In progress" }, { "label": "Review", "lane": "Frontend", "start": "2026-10-14", "end": "2026-10-18", "group": "Planned" }] }
```

## Images, models, and audio

Every `src` is a path relative to the project, an absolute path, a
`file://` URI, or an `http(s)` URL (images only).

### image

Show an image the reply is about: a screenshot you took, a plot you
rendered, a generated asset. A single image is centered over a blurred copy.
Use `src`, `alt`; or `images: ["path", ...]` (`paths` is an alias) for a
Discord-style gallery with balanced rows of two or three square tiles.
Entries can also be `{ src, alt?, caption? }`, mixed with plain paths.
Galleries support up to 16 images. Tiles keep the complete image over a
blurred background; click any tile for the full preview. Navigate the gallery
with the outside arrow buttons, Left/Right keys, or a horizontal drag.
`width?`, `height?`: the size hint used until a single image loads.

```artisan-image
{ "src": "docs/assets/after.png", "alt": "The settings page after the fix" }
```

```artisan-image
{ "images": ["docs/assets/front.png", "docs/assets/side.png", "docs/assets/back.png"] }
```

### image-compare

Two images of the same thing: a screenshot before and after a UI change, two
renders, two plots. Both share one frame at the wider image's ratio. The
reader can switch between a draggable slider, side by side, a click-to-flip
toggle, and a blend. `before`, `after`: `{ src, label? }` (both required);
`mode?`: `slider` (default), `side-by-side`, `toggle`, `blend`; `width?`,
`height?`: the size hint used until the images load.

```artisan-image-compare
{ "before": { "src": "docs/assets/before.png", "label": "Before" }, "after": { "src": "docs/assets/after.png", "label": "After" } }
```

### model

A 3D model the reader can turn: a mesh you exported, generated, or are
discussing. It draws on a transparent canvas in a neutral clay (or the
file's own material and vertex colours) under studio lighting. The reader
drags to orbit, Shift-drags to pan, Ctrl-scrolls to zoom, double-clicks to
reset, and switches between shaded and wireframe. Reads OBJ, binary FBX,
STL (binary and ASCII), PLY, and glTF: `.glb`, or `.gltf` with its buffers
embedded. Textures are not drawn. Local files only, at most 12 MiB.
`src` (required); `name?` (else the file name); `format?`: `obj`, `fbx`,
`stl`, `ply`, `gltf` (`glb`), when the content is ambiguous; `up?`: `y` or
`z`, when the model lies on its side (STL defaults to `z`, FBX reads its
own setting, the rest `y`); `color?`: a CSS colour for uncoloured surfaces;
`shading?`: `shaded` (default) or `wireframe`; `yaw?`, `pitch?`: the
opening view in degrees (default 30 and 18); `width?`, `height?`: the
frame's aspect (default 16:10).

```artisan-model
{ "src": "exports/gearbox.glb", "title": "The redesigned gearbox housing", "up": "z" }
```

### audio

Audio files the reader can play in place: a rendered sound, a recording, a
text-to-speech take. Each track is a row with a play button and a seekable
progress line; one track plays at a time. Local files only (mp3, wav, flac,
ogg). `src` (required unless `tracks`), `name`; or
`tracks: [{ src, name? }]`.

```artisan-audio
{ "tracks": [{ "src": "out/notify.wav", "name": "Notification, v2" }, { "src": "out/notify-old.wav", "name": "Notification, v1" }] }
```

### Colours are not a block

Write colour values straight into the reply. A value on its own (in a
sentence, a table cell, or alone in backticks: `#7c5cff`,
`oklch(0.7 0.15 250)`) draws as a colour square in place of the text; a
value inside longer code (`color: #333;`, a CSS fence) gets a square
before it. The reader right-clicks any of them to copy it as written or as
HEX, RGB, HSL, OKLCH, OKLab, or Display P3. Because a lone value shows only
its square, name the colour beside it when the reader needs the value too
(a palette table with a name column, or the value in a code snippet).
