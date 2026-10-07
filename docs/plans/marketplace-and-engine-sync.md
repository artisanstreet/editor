# Marketplace and engine extension sync

- Status: phase 1 implemented on branch `marketplace` (2026-10-07); see Implementation status
- Scope: one Artisan-owned registry of agent extensions (skills, MCP servers, subagents, hooks,
  instructions) and one Artisan-owned global prompt, both projected by the Forge into every
  managed engine (Claude Code, Codex, Cursor Agent, Grok, OpenCode2); the Marketplace screen that
  browses, installs, and manages extensions; the Settings field that edits the global prompt
- Builds on: `managed-engines.md` (Forge-owned binaries, homes, generations, `use.lock`)

## Outcome

Artisan is the single authority for what extensions an agent has. A user clicks Install once; the
Forge stores the package, records the desired state, and every engine sees it on its next session
without the user touching `~/.claude`, `config.toml`, `mcp.json`, or a vendor plugin command. The
Editor shows, per extension and per engine, whether it is applied, pending, unsupported, or
failed, and that status is confirmed by the engine's own report, not inferred from a file write.

The first consumer is the in-flight bundled `skills/artisan-editor` skill. Today it is untracked
and reaches Claude only through a hand-made symlink in the Claude engine home; Codex, Cursor, Grok,
and OpenCode2 never see it. Shipping it through this pipeline is the proof that the pipeline works.

## What exists today

- Engine homes are Forge-owned per engine profile: `<data>/toolchain/<engine>/home` (primary) with
  the child environment built explicitly (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `HOME`), spawned with
  `env_clear` (`modules/native_engine/src/claude.rs`, `codex.rs`, `install/spec.rs`). Artisan
  therefore already owns every path an engine reads extensions from.
- `install/spec.rs` already has the generation pattern (`versions/`, `selection.json`, exclusive
  `install.lock`, shared `use.lock` so a switch never lands mid-run). Extension sync reuses it.
- ACP engines (Grok, Cursor) open sessions with `session/new { cwd, mcpServers: [] }`
  (`engine_owner/acp/runtime.rs`): MCP injection there is a protocol field, not a file.
- Claude runs with `--setting-sources`, `--tools`, `--permission-mode`, etc. built in
  `engine_owner/claude/`; Codex runs `codex app-server --stdio`.
- Marketplace in the native app is only a placeholder: the inactive sidebar row
  (`native_application/impl_surfaces.rs`), the legacy rail button (`shell.rs`), the
  `marketplace.lifecycle` ledger kind (`subscription_patch_selection_policy.rs`), and the fixture
  command list (`marketplace_fixture_policy.rs`: routines, capabilities, npx-skills, OAuth).
- The TypeScript era had a full backend (`git show 6893f476d`, `a8d312887`, `c0da9abea`):
  capability and routine repositories, approval-gated operations, secure HTTP/stdio MCP
  transports, OAuth, a secret store, and per-engine "provider mirrors". The mirror was
  deliberately injectable and its only live implementation recorded `runtime_only`: it never
  wrote engine files. That is the gap this plan closes. The `marketplace_*` tables exist only in
  the legacy drizzle folder, not in `modules/migrations`.
- Evidence of the problem: the Claude and Codex homes each hold the same ten hand-copied skills
  (all dated 2026-09-28), and `~/.claude/CLAUDE.md` says it was "ported from Codex AGENTS.md".

## Principles

1. **Desired state lives in Artisan; engines get projections.** The Forge database and content
   store are the source of truth. Engine homes and launch arguments are derived output, rebuilt
   from desired state, never edited by hand by Artisan code paths that skip the registry.
2. **Prefer launch-time injection over writing engine files.** Where an engine accepts extensions
   per launch or per session (`--plugin-dir`, `--mcp-config`, `-c` overrides, ACP `mcpServers`,
   config search paths), use that: no drift is possible and project scoping is free. Materialize
   into the home only where the engine offers nothing else.
3. **Artisan owns only what it wrote.** Every materialized path is namespaced (`artisan/` subtree
   or an `artisan--` prefix) and recorded in the projection ledger. Anything else in an engine
   home is the user's (or the engine's own marketplace's): we observe and offer to adopt it, but we
   never delete or overwrite it. The one exception is each home's global instruction file, which
   Artisan takes over whole after a backed-up first-run import (see Global prompt).
4. **Immutable, content-addressed packages; atomic generations.** A package version is stored once
   by digest. Each sync builds a new per-engine-profile generation directory and swaps a pointer
   under the existing `install.lock`; runs hold `use.lock`, so a change applies to the next session
   and a running turn never sees a half-written set.
5. **Applied means the engine said so.** After a launch, read the engine's own inventory (Claude
   `system/init` lists skills, plugins, MCP servers; Codex app-server skill/MCP listing; ACP
   `available_commands_update`) and confirm each projected item. File writes alone are `pending`.
6. **Unsupported is a first-class status.** Not every engine supports hooks or subagents. The
   adapter says so, and the UI shows it instead of silently dropping the item.

## Extension kinds

| Kind | Canonical form in the store | Notes |
|---|---|---|
| Skill | `SKILL.md` folder (open standard) | Same bytes for every engine. |
| MCP server | Typed descriptor: `stdio {command,args,env}` or `http {url,headers}`, auth spec, tool list | Engines get a gateway URL (see MCP gateway), not the raw descriptor. |
| Bundle | Manifest listing member packages | Imports Claude `.claude-plugin`, Codex `.codex-plugin`, Cursor `.cursor-plugin`, Grok plugins; flattened into members. |
| Subagent | Markdown + frontmatter (Claude `agents/*.md` shape) | Translated per engine; unsupported where no equivalent. |
| Hook | Event + command | Per-engine event mapping; most often unsupported outside Claude and Grok. |
| Instruction | Markdown fragment | Appended to the global prompt's projection (see Global prompt), after the user's own text, in a delimited block per package. |

Vendor connectors (claude.ai connectors, Codex `.app.json` connector ids) cannot be synced: the
token never leaves the vendor account. They are shown as "connect in <vendor>" only.

## Architecture

```
 Catalog sources ──fetch──▶ Content store (by digest) ──▶ Registry (desired state, Forge DB)
 (MCP Registry, skills.sh,                                   │
  git marketplaces, local,                                   ▼
  first-party payload)                         Reconciler (per engine profile)
                                                │  plan = adapter.project(resolved set)
                                                ▼
                         Generation dir  toolchain/<engine>/extensions/<gen>/  (+ pointer swap)
                                                │
                         Launch contribution (args, env, ACP mcpServers) at spawn
                                                │
                                                ▼
                         Engine reports inventory ──▶ Projection ledger (applied / failed)
```

### Content store

`<data>/extensions/store/<sha256>/` holds one unpacked, verified package version: read-only,
written by stage-then-rename, never mutated. Fetching enforces the legacy inspector's rules (no
symlinks out of root, bounded file count and size, `SKILL.md` required for skills, safe relative
paths). Local dev sources (a repo folder like `skills/artisan-editor`) are a `linked` source: the
store entry is a pointer that re-digests on change, so editing the skill in the repo reaches
engines on the next session without a reinstall.

### Registry (Forge database, new `modules/migrations`)

Rework the legacy tables rather than port them verbatim:

- `extension_sources`: id, kind (`first_party | mcp_registry | skills_sh | git_marketplace |
  local_path`), locator, trust default, enabled, last refresh.
- `extension_packages`: id, source id, kind, name, publisher, display metadata.
- `extension_versions`: package id, version, digest, manifest json, fetched at. Immutable.
- `extension_installations`: package id, version policy (`pinned <v> | track latest`), scope
  (`user | project <root>`), enabled, engine selection (all or an explicit set), installed at.
- `extension_projections`: installation id, engine id, profile id, generation, method
  (`launch | materialized`), status (`pending | applied | unsupported | failed | drifted`),
  applied digest, observed digest, error code, updated at. This is the legacy `*_mirrors`.
- `extension_operations`: the approval-gated operation ledger (preview, approval fingerprint,
  decision, state, failure code). Same shape as the legacy `*_operations`; it feeds the existing
  `marketplace.lifecycle` subscription kind.
- `extension_secrets`: references only; values live in the OS keychain / Forge secret store.

### Engine adapters

One adapter per engine in the Forge, behind one trait:

```rust
trait EngineExtensionAdapter {
    /// What this engine version supports, and how (launch or materialized), per kind.
    fn support(&self, version: &EngineVersion) -> ExtensionSupport;
    /// Writes the generation directory for one profile from the resolved set.
    fn project(&self, set: &ResolvedExtensionSet, out: &GenerationDir) -> Result<Projection, ProjectionError>;
    /// Arguments, environment, and ACP session fields this generation adds at spawn.
    fn launch_contribution(&self, projection: &Projection, run: &RunContext) -> LaunchContribution;
    /// Reads the engine's own inventory (handshake or home scan) for confirmation and import.
    fn observe(&self, source: ObservationSource<'_>) -> ObservedInventory;
}
```

Projection per engine. Every row is verified by a contract test against the managed binary
before it ships; the per-version `support()` gate decides between the preferred and fallback
methods.

| Engine | Skills | MCP servers | Subagents / hooks |
|---|---|---|---|
| Claude Code | Generated `artisan` plugin in the generation dir, passed `--plugin-dir`; fallback: symlinks under `$CLAUDE_CONFIG_DIR/skills/artisan--*` | `--mcp-config <gen>/mcp.json` (not strict, so user servers remain) | Same generated plugin (`agents/`, `hooks/hooks.json`) |
| Codex | Materialized under `$CODEX_HOME/skills/artisan--*` | `-c mcp_servers.<id>.url=…` overrides on `codex app-server` launch | Subagents/hooks unsupported until Codex has an equivalent |
| Cursor Agent | Not placed while Cursor runs are unavailable in Artisan; materialized under `~/.cursor/skills/` once they are | ACP `session/new.mcpServers` | Unsupported |
| Grok (ACP) | `[skills] paths` in `~/.grok/config.toml` pointing at the generation dir (one managed line, then launch-stable) | ACP `session/new.mcpServers` | `[plugins]` path to a generated Grok plugin |
| OpenCode2 | `skills` array in global `opencode.json` pointing at the generation dir | `mcp` key in the same generated config, or ACP | Per OpenCode plugin support |

The global prompt and instruction packages have their own projection table under Global prompt.

### Cursor runtime

Decided 2026-10-07: Cursor stays on its CLI as it is. The Cursor SDK (`@cursor/sdk`) was
evaluated (system prompt option, per-session MCP, usage data) but it removes Artisan's approval
rail and adds a Node host; ACP as a general strategy is also out, because the native Claude and
Codex protocols carry more than ACP does. Cursor runs are not wired in the native app today
(`operation/cursor.rs` refuses every turn), so the sync reports Cursor as unavailable for skills
and the global prompt until they are.

### MCP gateway (Forge)

Engines never get third-party credentials. The Forge runs one loopback MCP gateway:

- Each enabled MCP installation is exposed as `http://127.0.0.1:<port>/mcp/<installation>`; each
  engine session gets a short-lived bearer token scoped to the installations its run may use.
- Stdio servers run once under Forge supervision (not once per engine session), with the
  environment the user approved and secrets injected from the keychain.
- Remote servers needing OAuth are authorized once in Artisan (the legacy OAuth begin/continue
  flow); the gateway attaches and refreshes tokens. One sign-in serves every engine.
- The gateway sees every tool call: per-tool allow/deny policy, health, latency, and call logs per
  run, the same observability for every engine.
- Every MCP server goes through the gateway, including unauthenticated public ones; there is no
  bypass. Every engine gets one transport type (HTTP), one policy point, and one log.
- The gateway owns tool exposure. Cursor's ACP mode loads every configured server's full tool
  definitions into each prompt (Cursor forum, 2026-09-12: ~259 tools, ~490k tokens, staff
  confirmed, workaround only). Because engines only see what the gateway advertises, it can cap
  that per engine: expose tools directly for engines that load them lazily, and for eager engines
  expose a compact `search_tools` / `call_tool` pair that resolves real tools on demand.

This is the vendor-neutral equivalent of "connectors". The cost is that the Forge sits in the MCP
hot path; it already owns engine lifecycles, so its availability is already a precondition.

### Reconciliation

- Triggers: an install, update, enable, or remove operation; an engine install or version switch
  (support can change); app update (first-party versions change); a linked local source changing;
  Forge start.
- Per engine profile: resolve the set (user scope plus, at launch, project scope for the run's
  root), call `project`, write `extensions/<gen>/`, swap the pointer under `install.lock`, record
  `pending`. Generations are retained like engine versions, so rollback is a pointer swap.
- At spawn: add `launch_contribution`; keep `use.lock` shared for the run.
- After the handshake: `observe`, then mark each item `applied` or `failed` with the engine's
  reason. Items the engine reports that Artisan did not project are "discovered" (see Import).

### Import and adoption

On first enable, and whenever observe finds unknown items, scan each engine home (skills dirs,
MCP config, vendor plugin caches). Deduplicate by content digest across engines (the ten
duplicated skills become ten packages, each already present in two engines). The user can adopt
them, which moves them into the store, installs them for all engines, and deletes nothing until a
confirmed adoption replaces each copy; or leave them as engine-only items.

### First-party bundled skills (the in-flight work)

- Move `skills/artisan-editor` into a first-party extension source shipped in the app payload
  (`modules/assets` manifest or a sibling `extensions/` payload directory), digest-pinned per
  build.
- The Forge registers the first-party source at start and keeps one installation per first-party
  package: enabled by default, trust `first_party`, version tracks the app build, user may
  disable.
- Dev builds register the repo folder as a `linked` source instead, so editing the skill reaches
  every engine on the next session. This replaces the manual symlink in the Claude home (remove
  it once phase 1 lands).
- The rich-block renderer tests that already cite the skill (`ui/src/markdown_renderer/rich/tests.rs`)
  get a companion test that every `artisan-<kind>` documented in the skill is a kind the renderer
  draws, so the bundled skill cannot drift from the renderer.

## Global prompt

One Artisan-owned global prompt (the engines call it "global instructions", "user memory",
`CLAUDE.md`, `AGENTS.md`, or "user rules") is edited in Artisan Settings and reaches every engine.
It uses the same store, reconciler, generations, and projection ledger as extensions; it is a
singleton the user writes rather than a package they install.

The idea already exists twice: the TypeScript era shipped it (`git show 636c90f6b`: canonical
guidance repository, native-file provider mirrors, first-run import; PRD section "Global Guidance
Settings"), and the native code still carries the vocabulary: `EngineCapabilityName::GlobalGuidance`
(`domain/src/engine_socket.rs`, currently `Unsupported` on every engine socket) and the
`guidance.canonical.updated`, `guidance.selection.required`, and `guidance.provider.reconciled`
subscription kinds (`backend/src/subscription_patch_selection_policy.rs`). This plan revives those
names rather than inventing new ones.

### Canonical value

- Stored as a versioned row in the Forge database (content plus digest plus edited-at). Every
  saved version is kept, so the Settings field has history and undo.
- The projected text is the user's prompt followed by the enabled instruction packages, each in
  a delimited block naming the package. The user's text always comes first.
- Precedence, documented per engine where it differs: engine safety and system prompt, then the
  Artisan global prompt, then project guidance (the repo's own `AGENTS.md` / `CLAUDE.md`, which
  engines read natively and Artisan does not touch), then thread guidance, then the user message.

### Projection per engine

The engine homes are Artisan-private (`toolchain/<engine>/home`), not the user's own `~/.claude`
or `~/.codex`. So after the first-run import, Artisan owns each home's global instruction file
outright, and the file is a whole-file mirror of the projected text. Run-time injection is the
fallback only for engines that have no global file.

| Engine | Method | Target |
|---|---|---|
| Claude Code | Native file | `$CLAUDE_CONFIG_DIR/CLAUDE.md` |
| Codex | Native file | `$CODEX_HOME/AGENTS.md`; an existing `AGENTS.override.md` takes precedence in Codex, so import it and then retire it into the backup |
| Grok | Native file | `~/.grok/AGENTS.md` in the engine home. Flip the Grok socket's `GlobalGuidance` capability once the adapter lands |
| OpenCode2 | Native file | `~/.config/opencode/AGENTS.md` in the engine home |
| Cursor | Unavailable | Cursor runs are not wired yet, and the CLI has no global instruction file (user rules live in the IDE account settings). Once runs are wired: a delimited preamble on each session's first prompt |

Writing goes through the generation pipeline: the projected file is built in the generation dir
and then copied (not symlinked; some engines refuse symlinked instruction files) to the target
under `install.lock`. The ledger records the written digest. On observe, a different digest is
drift: the engine or the user edited the file outside Artisan.

### Status and drift

Each engine shows one of `synced`, `applied at run time`, `unsupported`, `sync failed`, or
`drifted`. Drift never merges silently: Settings offers Import (the external edit becomes the new
canonical version and fans out to every engine), Overwrite (re-project Artisan's version), or
Ignore (keep the external edit for that engine only, shown as drifted until resolved).

### First-run import

Ported from the legacy PRD rules:

1. Scan every engine home's global file, and read-only the user's ambient files (`~/.claude/CLAUDE.md`,
   `~/.codex/AGENTS.md`, `~/.grok/AGENTS.md`, `~/.config/opencode/AGENTS.md`) as candidates.
2. None found: start with an empty prompt; every engine is ready to sync.
3. One unique value after normalization (trim, line endings): import it silently.
4. Several unique values: Settings asks the user to choose, showing engine, path, modified time,
   and a diff of each candidate. Nothing is written until they choose (`guidance.selection.required`).
5. Before the first overwrite, back up every engine file Artisan replaces into
   `<data>/guidance/backups/<engine>/<timestamp>.md`, recoverable from Settings.

Today the Claude home's `CLAUDE.md` is a hand-port of an empty Codex `AGENTS.md`; under these
rules it imports as the canonical value and Codex, Grok, and OpenCode2 receive it.

### Settings UI

Settings gets a Global prompt page: a markdown editor with save, version history, a row of
engine status chips below it (coloured per status, with the reason on hover), and the drift and
first-run choosers as inline sheets. There are no per-engine toggles by default; the legacy PRD
was explicit that this should not become a sync control panel. An engine can be excluded only
from its status chip menu.

## Catalog sources

| Source | Provides | Integration |
|---|---|---|
| First party (app payload) | Artisan's own skills and instructions | Built in; auto-installed |
| Official MCP Registry (`registry.modelcontextprotocol.io`) | MCP server descriptors with verified namespaces | Primary MCP feed; paged API, cached |
| skills.sh / `npx skills` | Skill packages | Primary skill feed (the legacy `DiscoverNpxSkills`/`ImportNpxSkills` commands) |
| Git marketplaces | Repos with `.claude-plugin/marketplace.json`, `.codex-plugin/`, `.cursor-plugin/` | User-added by URL; bundles flattened into members |
| Local path | A folder on disk | Dev and private skills; `linked` mode |

Third-party directories (Smithery, Glama, PulseMCP, mcp.so) mostly re-index the official registry;
add them only if a gap shows up. Trust is per source, with first party > verified registry
namespace > git marketplace > local; the review sheet always shows it.

## Marketplace screen

Entry is the existing sidebar Marketplace row, which becomes active and opens a full surface the
same way Settings does.

- **Discover**: search across sources, kind filter (Skills, MCP servers, Bundles, Agents, Hooks),
  featured and first-party shelves, source and trust badges, engine compatibility chips on every
  card.
- **Detail**: description and README, publisher and source, version history, exactly what it
  contains (files; MCP tools introspected through the gateway), what it needs (commands it runs,
  network, secrets, OAuth), per-engine compatibility with the reason for any unsupported engine.
- **Install review sheet**: scope (Everywhere or This project), engine toggles (default all
  supported), secrets inputs or a Connect button for OAuth, then Approve. This is the existing
  preview/approval operation; nothing is written before approval.
- **Progress and result**: per-engine rows move `pending → applied` live via the
  `marketplace.lifecycle` subscription; the copy says "Available in new threads" because a running
  session keeps its generation.
- **Installed**: a matrix of installed items × engines with status icons (applied, pending,
  unsupported, failed, drifted), coloured per state with matching icon colour. Row actions:
  enable/disable, per-engine toggles, update, roll back, remove. A "Found in your engines" group
  lists discovered items with Adopt and Ignore.
- **Updates**: first-party updates with the app; third-party track `latest` with an update badge,
  or pin. A version change that alters permissions goes back through review.
- **Sources**: list, add a git marketplace or local folder, refresh, disable.
- **In threads**: the composer's slash menu lists installed skills; a run's MCP calls show which
  installation served them (from gateway logs).

## Delivery phases

Each phase is shippable on its own and ends with an end-to-end test against real managed engine
binaries in a temporary data root.

1. **Pipeline, first-party skills, global prompt.** Store, registry migrations, reconciler with
   generations, Claude and Codex skill adapters, first-party source with `artisan-editor`,
   `linked` dev source, observe-based confirmation for Claude (`system/init`) and Codex. The global
   prompt rides the same reconciler: canonical store, Claude and Codex native-file projection,
   first-run import with backups, the Settings page. E2E: a fresh data root, both engines' first
   sessions list `artisan-editor` and answer with a marker phrase set only in the global prompt;
   remove the installation and change the prompt, and the next sessions reflect both.
2. **Every engine, Installed screen, import.** Cursor,
   Grok, and OpenCode2 skill and global-prompt adapters (Cursor through `systemPrompt` if the
   spike passes, else the preamble); the Marketplace surface with Installed and
   Sources tabs; home scan with digest dedupe and adoption; global prompt drift handling. E2E: the
   ten duplicated skills are adopted once and appear in all five engines, and all five answer with
   the global prompt's marker phrase.
3. **MCP through the gateway.** Gateway, secret store, OAuth flow, per-engine MCP injection (ACP
   `mcpServers`, Claude `--mcp-config`, Codex `-c`), health and call logs. E2E: one stdio server
   and one OAuth HTTP server, signed in once, callable from every engine; revoking access removes
   the tools on the next session.
4. **Catalog and Discover.** MCP Registry and skills.sh clients, git marketplaces (bundle
   flattening across vendor manifests), the Discover and Detail screens, install review, updates
   and rollback. E2E: install the Cloudflare bundle from its git marketplace and get its skills
   and MCP server in every engine.
5. **Subagents, hooks, instruction packages, project scope.** Per-engine translation where
   supported, instruction packages appended to the global prompt projection, project-scoped
   installations injected only for runs under that root, drift resolution UI for extensions.

## Testing

- Adapter contract tests per engine and supported version: project a fixture set into a temp
  profile, spawn the managed binary, and assert the engine's own inventory. These run in the
  managed-engines test lane, not on every unit test run.
- Reconciler property tests: any sequence of operations converges to the same generation as a
  fresh projection of the final desired state; a running `use.lock` never observes a partial
  generation.
- Ownership tests: a home seeded with user skills, MCP entries, and vendor plugins survives every
  Artisan operation byte for byte.
- Store tests reuse the legacy inspector cases (symlink escape, oversize, missing `SKILL.md`,
  unsafe paths).

## Decided

1. **Materialize by symlink on every platform, Windows included** (2026-10-07). There is no silent
   copy fallback. On Windows, unprivileged symlink creation
   (`SYMBOLIC_LINK_FLAG_ALLOW_UNPRIVILEGED_CREATE`) works only with Developer Mode on; otherwise
   it needs `SeCreateSymbolicLinkPrivilege` (administrator). The Forge probes this once at start
   by creating and removing a test link in its data root:
   - Probe passes: symlink as on Linux and macOS.
   - Probe fails: materialized projections for that platform go to `failed` with the typed error
     `symlinks_unavailable`; launch-time projections (Claude `--plugin-dir`, ACP `mcpServers`,
     Grok and OpenCode config paths) still work. Settings and the Marketplace show one banner,
     "Windows symlinks are off", with two actions: **Turn on Developer Mode** opens
     `ms-settings:developers`, and **Enable for me** runs a one-time elevated helper (UAC prompt)
     that sets `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock\AllowDevelopmentWithoutDevLicense = 1`.
     The Forge re-probes when the banner is dismissed or on the next start, then reconciles.
   - Not used: NTFS directory junctions. They need no privilege and would cover skill folders,
     but they are directory-only and local-volume-only, so they would add a second link kind to
     test.
   - The global prompt files stay copies everywhere (some engines refuse symlinked instruction
     files), so they never depend on the probe.
2. **Every MCP server goes through the gateway** (2026-10-07). No bypass; see MCP gateway.

## Decisions to confirm

3. **Coexist with engine-native installs or take exclusive ownership of engine homes.**
   Recommendation: coexist (principle 3) with adoption offered; exclusive ownership would break
   users who also run the CLIs directly against those homes.
4. **Project scope by launch injection only, or also writing into the repo** (`.claude/skills`
   etc.). Recommendation: launch injection only; writing into repos changes users' git trees.
6. **Whole-file ownership of engine global files.** Recommendation: yes, because the homes are
   Artisan-private; the user's ambient `~/.claude` and `~/.codex` are only ever read for import.

## Implementation status (2026-10-07)

Phase 1 is implemented on branch `marketplace`, with pieces of phase 2 that cost little once the
pipeline existed:

- `native_engine::extensions`: bounded skill reading and digests, the read-only content store,
  engine locations (Claude, Codex, Grok, `OpenCode2` profile homes; Cursor unavailable), owned
  links, prompt files, backups, and the symbolic-link probe.
- Migration `m20261007_000028_extensions_and_global_prompt` and repository slices.
- `backend::extension_sync`: the Forge-side reconciler (startup, every change, every 30 s),
  first-party `artisan-editor` (embedded in release, linked from the repository in debug),
  discovery grouped by content, adoption with backups, the global prompt with setup import,
  choice, drift, import, and overwrite.
- Protocol: reads, commands, answers, and pushed events for the catalog and the global prompt.
- Editor: the Marketplace route from the sidebar, the Settings global prompt page, and a reusable
  `NativeTextEditor`.

Deviations from the design above, by decision while building:

- Skills are placed by symbolic link for every engine, named exactly as the skill, instead of
  Claude's `--plugin-dir`, which would namespace every skill (`artisan:artisan-editor`).
  Ownership is the link target: a link into the store (or the repository's `skills/` in debug)
  is Artisan's; everything else is the user's.
- "Applied" is confirmed by the files in each engine home, not yet by each engine's own
  inventory (Claude `system/init` does not parse `skills` today); confirming through the engines
  stays in phase 2.
- On Windows without Developer Mode the Marketplace shows the banner and opens Developer
  settings; the elevated "Enable for me" helper is not built yet.

Not started: the MCP gateway and secrets (phase 3), catalog sources and Discover (phase 4),
subagents, hooks, instruction packages, and project scope (phase 5).

