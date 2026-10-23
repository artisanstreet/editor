---
name: artisan-editor
description: Write replies that render richly in the Artisan Editor. The Editor draws `artisan-<kind>` fenced JSON blocks as native interactive widgets — charts, stat strips, heatmaps, distributions, funnels, sankeys, treemaps and flame graphs, Gantt charts and waterfalls, comparisons, callouts, clickable code references, annotated code, file trees, images, image comparisons, 3D model viewers, and audio players; colour values written in a reply copy in every notation on right-click. Use whenever you answer inside the Artisan Editor and the reply reports data or measurements, compares options, points at code, shows or compares images, shows a 3D model, plays audio, or shows colours. Triggers on "visualize", "chart", "graph", "dashboard", "show the data", "compare", "how lopsided", "where is", "what does this look like". Also covers the `artisan` MCP tools for searching other threads ("like we did in the other thread"), running agents on any harness ("have Codex review this") and changing Artisan settings and hosts.
---

# Artisan Editor rich blocks

The Artisan Editor is a native app, not a browser. It does not render HTML.
Instead it draws **rich blocks**: fenced code blocks whose info string is
`artisan-<kind>` and whose body is a single JSON object. Each kind is a native
widget with built-in interaction (hover readouts, toggles, zoom, sliders). Anything the Editor cannot draw falls back to the code itself, so a
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
| Report headline numbers | `stats` |
| Show a trend, share, or relationship over values | `chart` |
| Show a value over two dimensions (weekday × hour) | `heatmap` |
| Show the shape of many values | `distribution` |
| Show how lopsided a total is ("top 10% send 65%") | `concentration` |
| Show drop-off through steps | `funnel` |
| Show flows between categories | `sankey` |
| Show what a total is made of, nested (disk use, bundle size, a profile) | `hierarchy` |
| Show intervals over time (a plan, a request waterfall, trace spans) | `spans` |
| Compare options side by side | `compare` |
| Flag a risk, decision, warning, or assumption | `callout` |
| Point at code the reader can open | `code-ref`, `annotated-code`, `file-tree` |
| Show an image (a screenshot, a plot, a generated asset) | `image` |
| Compare two images | `image-compare` |
| Show a 3D model the reader can turn (OBJ, FBX, STL, PLY, glTF) | `model` |
| Let the reader play audio files | `audio` |
| Show colours or design tokens | No block: write the values (`#7c5cff`, `oklch(…)`) in prose or a Markdown table |

**Never restate what the transcript already shows.** File changes, plans and
todos, commands and their output, test runs, and questions to the reader
appear on their own from what you actually did; do not redraw them as blocks.
Draw diagrams as `mermaid` fences and tables as Markdown tables: both render
natively.

Do not use a block for one sentence, for a list prose handles fine, or to
decorate. Two or three well-chosen blocks beat ten. Never wrap a whole reply
in blocks.

## Rules

1. **One JSON object per fence**, info string `artisan-<kind>` in lowercase.
   Valid JSON only: double quotes, no comments, no trailing commas.
2. **Envelope fields** work on every kind: `title` (the finding, stated as a
   sentence), `subtitle` (one muted line of scope and units: "Share of sent
   turns per day, by model family. Jul 6 to Oct 2"), and `caption` (a muted
   line below, for provenance).
3. **Plain text in strings.** No Markdown inside JSON strings; `body` fields
   take blank-line-separated paragraphs (`\n\n`).
4. **Data is inline and real.** Charts carry their numbers. Never invent data
   to fill a chart; if you only have three numbers, use `stats`.
5. **Refs are project paths**: `"src/a.rs"`, `"src/a.rs:42"`,
   `"src/a.rs:42-60"`. The reader can click them to open the file.
6. **Shared vocabulary.** `tone`: `neutral`, `info`, `success`, `warning`,
   `danger`, `accent`.
7. **Keep it small.** A block body is capped at 256 KiB; a chart reads best
   under ~2000 points and eight series.
8. **Write the block after you know the content.** The block draws once its
   fence closes; while streaming the reader sees a placeholder.
9. **Media paths are project paths** like refs. Images may also be absolute
   paths or URLs; audio and models load only from files.

## Composition patterns

- **Data answer:** `stats` (three to five headline numbers) → one `chart` per
  question, each with a title that states the finding ("Opus 5.5 went from 0
  to half of all turns in 3 days") and an `annotations` marker for the event
  that explains it → a Markdown table for the raw rows if the reader may want
  them.
- **Design choice:** a paragraph of context → `compare` → a `callout` naming
  the recommendation and its main risk.
- **Code walkthrough:** prose → `annotated-code` at the part that matters →
  `code-ref` for the related places.
- **Visual change:** `image-compare` of before and after → a Markdown table
  of the colours that changed, values written out.
- **Show what you made:** a screenshot, plot, or generated image as `image`;
  a mesh you exported or generated as `model`; rendered sounds as `audio`.

Titles state findings, not topics: "Peak is Thursday 3 PM, local time" beats
"Turns by hour".

## Artisan's own tools

Every thread in Artisan has an MCP server named `artisan` (Claude shows its
tools as `mcp__artisan__*`). It acts for the thread you run in, so "this
thread" and "my agents" need no ids.

**Recall other threads.** When the user points at earlier work ("like the
fix in the sidebar thread", "what did we decide about auth"), look it up
instead of asking:

- `threads_search` ranks every thread's messages, plans, commands, errors and
  file paths. Start here with a few distinctive words.
- `threads_grep` runs a regex line by line, newest first, like `rg`.
- `thread_read` reads the items around a hit (`thread` + `item`), or a
  thread's newest page; `whole=true` reads one long item in full.
- `threads_touching` lists the threads that read or changed a file.
- `thread_list` lists threads by project, state or title.

Tool output is left out unless you pass `include_output=true`; failed
commands always show theirs. Quote what you found and name the thread.

**Agents on any harness.** `harnesses` lists each harness (claude, codex,
opencode, grok, cursor) with its models, efforts and sign-in. `agent_spawn`
starts one on any of them. It sees only your `prompt`, so make the prompt the
whole brief: goal, files, constraints, what to report. Settings you leave out
come from this thread; a fallback is named in `warnings`. Then either
`agent_wait` (up to 9 minutes per call, call again to keep waiting), or pass
`background=true` and keep working: the result arrives as a message starting
`[Artisan]`. `agent_message`, `agent_read`, `agent_cancel` and `agent_list`
manage them. An agent never gets more permission than this thread, and the
user sees it, with its harness logo, in the Agents panel.

**The app itself.** `thread_create`, `thread_send`, `thread_cancel`,
`thread_update`, `requests_list`; `settings_list` / `settings_get` /
`settings_set`; `hosts_list`, `host_add`, `host_remove`, `host_switch`;
`projects_list`, `project_add`, `project_remove`; `mcp_servers_*`,
`skills_list`, `skill_set_enabled`, `global_prompt_get` / `global_prompt_set`,
`harness_set_default_model`, `harness_install`; `open_thread`,
`open_settings`. Do what the user asked and no more: tools that change a
page take `navigate_user`; set it only when the user asks to be taken there.

- Removing something, adding an MCP server, replacing the global prompt and
  installing a harness show the user an approval card first. If they decline,
  the tool says so; don't retry it.
- Host, Editor-setting and `open_*` tools need the user's Editor open; if
  none answers, say so rather than retrying.
- An HTTP MCP server that fails with HTTP 401 needs a sign-in:
  `mcp_server_sign_in` returns the link to give the user, and Artisan keeps
  the token for every harness. Don't send the user to settings for it.
- Tools that change something take `navigate_user`; set it only when the
  user asked to be taken to the page. Otherwise their screen stays put.
- An HTTP MCP server that needs signing in shows `needs sign-in` in
  `mcp_servers_list`. `mcp_server_sign_in` starts the sign-in and returns the
  link to give the user; with `navigate_user` it also opens the browser and
  the server's card. Never send the user looking for it in settings.
- Pairing a remote host needs the address, code and fingerprint that
  `ae pair` shows on that machine; ask the user for them.
- Only the user answers approvals and questions; `requests_list` only shows
  what is waiting.

**Installing skills and MCP servers.** Artisan owns every harness's skills
and MCP servers and keeps them the same in all of them. Install through it:

- A skill: write the folder (a `SKILL.md` with `name` and `description` front
  matter, plus any files it needs) somewhere outside the harness's config,
  such as the project or a temp folder, then call `skill_install` with its
  absolute path. Every harness gets it from the next thread on. To change it
  later, edit your copy and call `skill_install` again with `replace=true`.
- An MCP server: `mcp_server_add` with a command or a url.
- Check `skills_list` or `mcp_servers_list` first; if it's already there,
  turn it on with `skill_set_enabled` rather than installing a second copy.

Never write into a harness's own skills folder (`~/.claude/skills`,
`~/.codex/skills` and the like), and never run `claude mcp add`,
`codex mcp add` or edit their config files for MCP servers. Artisan removes
anything it didn't install there, and the other harnesses never see it.
Skill-writing guides that say to save into those folders mean
`skill_install` here.
