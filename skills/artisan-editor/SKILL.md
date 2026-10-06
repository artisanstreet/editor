---
name: artisan-editor
description: Write replies that render richly in the Artisan Editor. The Editor draws `artisan-<kind>` fenced JSON blocks as native interactive widgets — charts, stat tiles, heatmaps, tables, flows, steps, cards, callouts, comparisons, decision records, code references, annotated code, diffs, file trees, sequence and state diagrams, checklists, verification receipts, risk lists, choices, perf tables, logs, images with callouts, math, and more. Use whenever you answer inside the Artisan Editor and the reply walks through a change, explains architecture or a process, reports data or measurements, compares options, asks the reader to pick, or summarizes verification. Triggers on "walk me through", "visualize", "chart", "graph", "dashboard", "summarize the PR", "explain the flow", "compare", "what changed", "show the data", "how does X reach Y".
---

# Artisan Editor rich blocks

The Artisan Editor is a native app, not a browser. It does not render HTML.
Instead it draws **rich blocks**: fenced code blocks whose info string is
`artisan-<kind>` and whose body is a single JSON object. Each kind is a native
widget with built-in interaction (hover readouts, toggles, sorting, expanding,
sliders). Anything the Editor cannot draw falls back to the code itself, so a
mistake is never fatal, but it is ugly — get the JSON right.

````markdown
```artisan-callout
{ "kind": "risk", "title": "Breaking change", "body": "The CLI now rejects labels it used to accept." }
```
````

The full catalog with every field and an example per kind is in
[references/blocks.md](references/blocks.md). Read it before writing a kind
you have not used in this conversation.

## When to reach for a block

Prose stays the backbone. A block earns its place when it shows something
prose would bury:

| The reply needs to… | Use |
|---|---|
| Frame a walkthrough (PR, incident, investigation) | `header`, then prose and blocks |
| Flag a risk, decision, warning, or assumption | `callout` |
| Show parallel things side by side | `cards`, `before-after`, `compare` |
| Show how something flows or forks | `flow`, `sequence`, `state-machine` |
| Explain an ordered process | `steps` |
| Record a decision and its alternatives | `decision` |
| Point at code | `code-ref`, `annotated-code`, `diff`, `file-tree` |
| Describe structure | `architecture`, `symbol-graph`, `schema`, `api` |
| Report numbers | `stats`, `chart`, `heatmap`, `distribution`, `funnel`, `sankey`, `table`, `perf` |
| Report agent work | `checklist`, `progress`, `verification`, `test-matrix`, `risks`, `log`, `command` |
| Ask the reader to pick | `choice` |
| Show visuals | `image`, `image-compare`, `swatches`, `math` |
| Small labels and sources | `chips`, `footnotes`, `glossary` |
| Offer alternative views of one thing | `tabs` (each tab may hold a nested block) |

Do not use a block for one sentence, for a list prose handles fine, or to
decorate. Two or three well-chosen blocks beat ten. Never wrap a whole reply
in blocks.

## Rules

1. **One JSON object per fence**, info string `artisan-<kind>` in lowercase.
   Valid JSON only: double quotes, no comments, no trailing commas.
2. **Envelope fields** work on every kind: `title` (heading above) and
   `caption` (muted line below). Use `caption` for provenance ("Source:
   telemetry, Jul 6 – Oct 2").
3. **Plain text in strings.** No Markdown inside JSON strings; `body` fields
   take blank-line-separated paragraphs (`\n\n`).
4. **Data is inline and real.** Charts carry their numbers. Never invent data
   to fill a chart; if you only have three numbers, use `stats`.
5. **Refs are project paths**: `"src/a.rs"`, `"src/a.rs:42"`,
   `"src/a.rs:42-60"`. The reader can click them to open the file.
6. **Shared vocabulary.** `tone`: `neutral`, `info`, `success`, `warning`,
   `danger`, `accent`. `status`: `done`, `active`, `pending`, `failed`,
   `skipped`, `blocked` (`pass`, `fail`, `skip`, `warn` also read).
7. **Keep it small.** A block body is capped at 256 KiB; a chart reads best
   under ~2000 points and eight series.
8. **Write the block after you know the content.** The block draws once its
   fence closes; while streaming the reader sees a placeholder.
9. **`choice` ends your turn.** Picking an option sends the reader's answer
   as their next message. Put the choice last.
10. **Copy works.** Every block copies as Markdown, so the reply still reads
    if pasted elsewhere.

## Composition patterns

- **PR walkthrough:** `header` → one paragraph → `flow` (one source, two
  paths) → `cards` for the consumers → `steps` for how a change reaches users
  → `verification` → `risks` if any.
- **Data answer:** `stats` (three to five headline numbers) → one `chart` per
  question, each with a title that states the finding ("Opus 5.5 went from 0
  to half of all turns in 3 days") and an `annotations` marker for the event
  that explains it → `table` for the raw rows if the reader may want them.
- **Design choice:** a paragraph of context → `compare` or `decision` →
  `choice` asking the reader to pick.
- **Debugging report:** `log` (trimmed, with `highlight`) → `annotated-code`
  at the fault → `diff` of the fix → `test-matrix` or `verification`.

Titles state findings, not topics: "Peak is Thursday 3 PM, local time" beats
"Turns by hour".
