# Rich blocks

Agents write rich, interactive content into a reply without HTML. A rich
block is a fenced code block whose info string is `artisan-<kind>` and whose
body is one JSON object. The transcript draws it as a native GPUI widget in
the theme's own tokens; anything it cannot draw stays an ordinary code fence.

````markdown
```artisan-callout
{ "kind": "risk", "title": "Breaking change", "body": "The CLI rejects labels it used to accept." }
```
````

## Why not HTML

The Editor is native GPUI. An embedded web view would add a second visual
system beside the glass materials, a sandbox for agent-authored scripts, and
a seam where selection, scrolling, find, and reduced motion stop working.
Rich blocks keep one renderer: declarative specs, native widgets, theme
tokens, no scripts.

## Contract

- **Fence.** Info string `artisan-<kind>`, lowercase. The body is one JSON
  object, at most 256 KiB.
- **Streaming.** While the fence is open the block shows a quiet placeholder
  naming the kind; it draws once the fence closes.
- **Failure.** A closed fence whose JSON does not parse, or whose fields do
  not match the kind, renders as the code it is with one muted line naming
  the problem. An unknown kind renders as code. Nothing demotes the
  surrounding message.
- **Envelope.** Every kind accepts these optional fields:
  - `title` — a heading drawn above the block.
  - `caption` — a muted line drawn below it.
- **Leniency.** Unknown fields are ignored. Optional fields have defaults.
  Required fields are marked below.
- **Text.** String fields are plain text. Backticks are kept literally.
  `body` fields allow blank-line-separated paragraphs.
- **Chrome.** Most blocks sit directly on the transcript, framed by
  whitespace and their headline, with no card and nothing nested in boxes.
  Only the code kinds (`code-ref`, `annotated-code`) take the fence's glass
  card, like any code.
- **Copy.** Only code is worth copying: the code kinds carry the fence's
  copy action and copy their code, never the JSON. Charts, tables, trees,
  and images have no copy action. Where one value is worth taking (a color,
  an image's path), a right-click offers it.
- **Interactivity is declared, never scripted.** Hover readouts, toggles,
  sorting, expanding, and sliders are built into each kind. Interaction
  state lives in the window, keyed by the block's position in the message,
  and is not persisted.
- **Data is inline.** Charts carry their numbers in the spec; nothing is
  fetched. Images, models, and audio are the only external references (`src`): a
  path relative to the open project, an absolute path, `~/…` for the home
  folder, a `file://` URI, a `data:` URL, or (images only) an `http(s)` URL.
  Paths name files on the Forge's machine, where the agent that wrote them
  runs, and resolve by that machine's rules wherever the Editor runs.

### Shared vocabulary

| Name | Values |
|---|---|
| `tone` | `neutral` (default), `info`, `success`, `warning`, `danger`, `accent` |
| `ref` | `"path/to/file.rs"`, `"path/to/file.rs:42"`, or `"path/to/file.rs:42-60"` |

Colors come from the theme: tones map to the banner tokens. The theme's
`--chart-*` tokens are neutral steps, so series use a categorical palette of
eight hues whose lightness follows the theme mode (`kit::series_color`).

### Host actions

Some interactions reach outside the transcript. The UI crate dispatches GPUI
actions and the host decides what they do:

| Action | Fired by | Host behavior |
|---|---|---|
| `rich_blocks::OpenReference { path, line }` | `code-ref`, `annotated-code`, `file-tree` | Open the file in the thread's editor surface (the route has no line yet, so it opens at the top) |
| `rich_blocks::ShowCopyMenu { position, entries }` | right-click on a colour value in a reply, an `image`, or a `model` | Show the app's pointer menu (the thread row's glass menu): primary entries with the copy icon, then, past a separator, alternative forms indented without one |

The Editor handles both on its root (`native_application/impl_rich_blocks.rs`,
`impl_pointer_menu.rs`). Without a handler, the block copies the first text
to the clipboard instead. The host also installs a `MediaFetcher` for the
open thread: image, model, and audio files load through it with the
`ReadMediaFile` query, which the Forge resolves against the thread's project
or its user's home folder and answers with at most 12 MiB of image, audio,
or model bytes (text model formats must also carry their extension)
(`backend/src/media_file_service.rs`). Without a fetcher (tests, previews)
files are read on the Editor's machine against `MediaRoot`, the open
project. Fetched and `data:` audio plays from memory: the Editor writes no
files of its own.

## Catalog

The catalog is deliberately small. A kind exists only when Markdown and
`mermaid` fences cannot express it and the transcript does not already show
the real thing: file changes, plans, commands, test runs, and questions come
from the engine's own observations, never from a block restating them.

### Structure

**`callout`.** A note set apart. `kind`: `note` (default), `tip`, `warning`,
`risk`, `decision`, `assumption`; `body` (required); `collapsed: bool`.
The kind picks the tone and the label; collapsed callouts expand on click.

**`compare`.** Options against criteria, as a plain table.
`options: [string]` (required), `rows: [{ criterion, values: [string] }]`
(required), `recommended?: string` (an option). The recommended option is
named in the accent above its column; hovering a column highlights it.

### Code and repository

**`code-ref`.** A pointer into the code, on one glass card. `ref`
(required), `language?`, `excerpt?`, `note?`. Clicking the path opens it;
the excerpt is numbered from the ref's first line and sits directly on the
card. Copy copies the excerpt, else the ref.

**`annotated-code`.** Code with numbered notes, on one glass card. `code`
(required), `language?`, `path?`, `start_line?` (default 1),
`notes: [{ lines: "3-5" | 4, text }]`. A badge marks the first line each
note covers and the notes follow the code as plain numbered lines; hovering
a note highlights its lines and the reverse. Copy copies the code.

**`file-tree`.** Files touched. `entries: [{ path (required), change?: added|modified|deleted|renamed, note? }]`.
Directories are derived from the paths and collapse on click. Changes read
as colored letters and counts, not pills.

### Data and charts

**`stats`.** A row of figures.
`tiles: [{ label (required), value (required), delta?, trend?: up|down|flat,
good?: up|down, sparkline?: [number], hint? }]`. The delta is colored by
whether its trend is good; hovering a tile shows its hint.

**`chart`.** Line, area, bar, and scatter.
- `mark`: `line` (default), `area`, `bar`, `scatter`.
- `stacked`, `normalize` (stacked shares of 100%): booleans.
- `categories?: [string]` — category x values; series then give `values`.
- `series: [{ name, values?: [number|null], points?: [[x, y]] }]` (required).
- `x?: { label?, unit? }`, `y?: { label?, unit?, min?, max? }`.
- `annotations?: [{ x?, y?, x_end?, label }]` — a vertical rule (`x`),
  a horizontal rule (`y`), or a band (`x` to `x_end`).
- `params?: [{ id, kind: threshold, label, min, max, step?, value }]` —
  a slider that moves a horizontal rule and reports the share of points at or
  above it.

Hovering reads out every series at the nearest x. Legend entries toggle
their series. Dragging across the plot zooms to that x range; double-click
resets.

**`heatmap`.** A grid of values. `x: [string]`, `y: [string]`,
`values: [[number|null]]` (rows follow `y`; required), `unit?`,
`scale?: sequential|diverging`. Hovering a cell reads it out.

**`distribution`.** The shape of values. `mode`: `histogram` (default),
`box`, `cdf`; `series: [{ name, values: [number] }]` (required), `bins?`,
`unit?`, `threshold?: number`. In histogram and cdf modes a slider moves
the threshold and reports the share at or below it.

**`funnel`.** Conversion through steps.
`steps: [{ label, value }]` (required). Hovering a step reads the drop
from the one before.

**`sankey`.** Flows between categories. `nodes: [{ id, label? }]`,
`links: [{ from, to, value }]` (required). Hovering a node or link
highlights its flows.

**`hierarchy`.** A tree of sized parts. The spec is the root node,
`{ name?, value?, children? }`; a node's size is the larger of its `value`
and its children's sum. `layout`: `treemap` (squarified, two levels below
the focus), `icicle`, or `flame` (six levels); `unit`, `depth`. Cells and
bars are divs, so each hovers and clicks on its own; clicking a group zooms
in and the path above the plot zooms out.

**`spans`.** Intervals on a time axis. `spans: [{ label, start, end, lane?,
group? }]`, times all numbers (in `unit`) or all ISO dates. Without lanes,
one row per span (a waterfall); with lanes, overlapping spans in a lane
stack beneath it, earlier and longer first, so trace spans read as a flame
chart. `markers: [{ at, label }]` draw dashed rules.

### Images, models, and audio

**`image`.** Images shown as they are. `src` and `alt`, or
`images: [{ src, alt?, caption? }]`; `width?`/`height?` hint a single
image's size until it loads. An image keeps its aspect and pixel width,
capped at 480 px tall (240 px when several share a row), with a hairline
edge. An image that cannot load is one muted line naming it. Right-click
copies its path. The transcript draws generated engine images (below) with
the same view, `ImageShowcase`.

**`image-compare`.** Before and after. `before`, `after`: `{ src, label? }`,
`mode?` (`slider`, `side-by-side`, `toggle`, `blend`), `width?`/`height?`.
Both images share a frame at the wider pixel ratio, capped at 480 px tall.
A side that cannot load replaces the frame with a line naming it.
The reader switches modes below the frame; arrow keys move the divider or
blend, double-click resets it.

**`model`.** A 3D model to turn, on a transparent canvas. `src`
(required), `name?`, `format?` (`obj`, `fbx`, `stl`, `ply`, `gltf`/`glb`;
otherwise recognized by content), `up?` (`y` or `z`; STL defaults to `z`,
FBX reads its `UpAxis`), `color?` (CSS, for uncoloured surfaces),
`shading?` (`shaded`, `wireframe`), `yaw?`/`pitch?` (the opening view in
degrees), `width?`/`height?` (the frame's aspect, default 16:10, capped at
480 px tall). The model is centred and scaled into the unit sphere and
drawn in clay, or the file's material and vertex colours, under a studio
light rig. Where the renderer is wgpu (Linux, Windows) the block paints it
with GPUI's `Window::paint_mesh`: the geometry uploads once and every frame
only moves the camera, rendered offscreen with 4× MSAA and a depth buffer
and composited like an image. On macOS's Metal renderer a software
rasterizer draws it on the background executor instead (2×2 samples per
pixel at rest, one while dragging). Either way it lands straight-alpha on
the transcript. Drag orbits,
Shift-drag or the middle button pans, Ctrl-scroll (⌘ on macOS) zooms
while a plain scroll keeps scrolling the transcript, double-click resets;
once pressed, arrows orbit, `+`/`-` zoom, and `0` resets. The row below
switches shading and names the format and triangle count. FBX is binary
7.x only (ASCII FBX says so); glTF reads `.glb` and `.gltf` with embedded
buffers (a separate `.bin` says to export a `.glb`); textures and
animation are not drawn. A model that cannot load is one muted line naming
it and why. Right-click copies its path.

**`audio`.** Local audio files to play in place. `src` and `name`, or
`tracks: [{ src, name? }]`. Each track is a row: a play button, its name,
a progress line that seeks on click or drag, and its time. Playback is
app-wide, one track at a time (`artisan-audio`: symphonia decoding; the
PulseAudio protocol on Linux, so PipeWire and WSLg work without ALSA;
cpal elsewhere). URLs do not play.

### Colour values

Colour is not a block. Colour values a reply writes draw as swatches and
answer a right-click:

- **Bare in prose** (`Coral (#FF6B6B) is closest`): the value becomes a
  square alone, rounded with a hairline edge, held to its neighbours so
  `(■)` never wraps apart.
- **A code span holding only the value** (`` `oklch(0.7 0.15 250)` ``):
  a square alone, without the code chip.
- **Inside longer code**, inline or fenced (`` `color: #333;` ``, a CSS
  block): a square before the value, which stays, the way an editor
  decorates CSS colours. The fence's copy action copies the source as
  written.

Found: hex (`#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`) and CSS colour
functions (`rgb()`/`rgba()`, `hsl()`/`hsla()`, `hwb()`, `lab()`, `lch()`,
`oklab()`, `oklch()`, `color(display-p3 …)`, `color(srgb …)`) that parse.
In prose a hex value needs six or eight digits or a letter, so `#502`
stays an issue number; in code any length counts. Named colours are
words, never values; values inside links stay the link's text.

Right-clicking a swatch (or a decorated value) opens "Copy Value" with the
copy icon, then, past a separator, "Copy as HEX", RGB, HSL, OKLCH, OKLab,
and Display P3, indented as if they had an icon. HEX, RGB, and HSL clip a
colour outside sRGB into it; OKLCH, OKLab, and Display P3 keep it. A text
selection copies a lone swatch as its value and a decorating one as
nothing, so copied prose reads as written.

Swatch slots are inserted while the text is flattened
(`inline::flatten_inline_with_swatches`, fences in
`prepared::decorate_colors`), before any range exists, and
`SelectableText` paints the squares (`with_inline_swatches`) and maps the
slots back when copying (`with_copy_substitutions`). The frozen
`present_inline` contract keeps colours as text.

## Engine images

An image appears in the transcript because an agent chose to show it: an
`artisan-image` (or `artisan-model`) fence. Images an engine merely looked
at while working (a `Read` of a PNG, an MCP screenshot, Codex's
`imageView`) are steps of its work and draw nothing; the work section
already lists the step.

The one exception is an image the engine generated, which is the result
itself and whose path the agent may not know: Codex `imageGeneration` items
(from `savedPath`, else the base64 `result`; failed generations draw
nothing) arrive as observations, captioned with the revised prompt, and
draw with the `image` block's view, `ImageShowcase`, after the turn's reply
and outside its work section.

`Observation::Image` carries an absolute path and a caption, never pixels.
A generated image sent inline is written once, named by its SHA-256, under
`<forge state>/engine-images/` (8 MiB cap, PNG, JPEG, GIF, and WebP only).
Runs recorded before this rule may hold observations of looked-at images;
the Editor skips them.

## Known limits

- Web images load only once the host installs an HTTP client.
- `engine-images/` is never pruned.
- Media paths are read by the Editor. With a Forge on another machine,
  project-relative paths and engine image paths name files the Editor
  cannot read, and the block says it couldn't load them.
- `OpenReference` opens the file at its top: the editor route carries no line.
- macOS draws models on the CPU until the Metal renderer gets a mesh pass;
  large meshes turn at a lower frame rate there.

## Implementation

- `modules/ui/src/markdown_renderer/rich/` holds the blocks, one file per
  kind or family of kinds.
- `PreparedFence` parses a closed `artisan-*` fence once, when the message
  body enters the parse cache; frames reuse the parsed block.
- Each block implements `RichBlock`: it renders from a `BlockContext` (theme
  and stable element id) and renders itself as Markdown for copy.
- Agents learn the catalog from the `artisan-editor` skill in
  `skills/artisan-editor/`.
