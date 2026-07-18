<script lang="ts" effect>
	import { Effect } from "effect";
	import {
		IconAlertTriangle as AlertTriangle,
		IconCheck as Check,
		IconCircleFilled as CircleFilled,
		IconCommand as Command,
		IconDots as Dots,
		IconFile as File,
		IconGitBranch as GitBranch,
		IconInfoCircle as InfoCircle,
		IconLayoutSidebarRight as LayoutSidebarRight,
		IconLock as Lock,
		IconMessage as Message,
		IconMoon as Moon,
		IconPlayerPlay as PlayerPlay,
		IconPlus as Plus,
		IconSearch as Search,
		IconSettings as Settings,
		IconSparkles as Sparkles,
		IconSun as Sun,
		IconTerminal2 as Terminal2,
		IconX as X,
	} from "@tabler/icons-svelte";
	import { Button } from "$lib/components/ui/button";

	type FixtureTheme = "dark" | "light";
	type FixtureMode = "editor" | "chat" | "orchestrator";
	type FixtureFile = "agent" | "transport" | "readme";
	type PermissionResult = "pending" | "approved" | "denied";

	let fixture_theme: FixtureTheme = $state("dark");
	let high_contrast = $state(false);
	let reduced_motion = $state(false);
	let zoomed = $state(false);
	let long_labels = $state(false);
	let menu_open = $state(false);
	let active_mode: FixtureMode = $state("editor");
	let active_file: FixtureFile = $state("agent");
	let permission_result: PermissionResult = $state("pending");
	let activity_message = $state("No local fixture action yet");
	let menu_trigger: HTMLButtonElement;

	const SetTheme = (theme: FixtureTheme) =>
		Effect.gen(function* () {
			fixture_theme = theme;
		});

	const ToggleContrast = Effect.gen(function* () {
		high_contrast = !high_contrast;
	});

	const ToggleMotion = Effect.gen(function* () {
		reduced_motion = !reduced_motion;
	});

	const ToggleZoom = Effect.gen(function* () {
		zoomed = !zoomed;
	});

	const ToggleLabels = Effect.gen(function* () {
		long_labels = !long_labels;
	});

	const ToggleMenu = Effect.gen(function* () {
		menu_open = !menu_open;
	});

	const CloseMenu = Effect.gen(function* () {
		menu_open = false;
		yield* Effect.sync(() => menu_trigger.focus());
	});

	const SelectMode = (mode: FixtureMode) =>
		Effect.gen(function* () {
			active_mode = mode;
			activity_message = `Selected ${mode} fixture`;
		});

	const SelectFile = (file: FixtureFile) =>
		Effect.gen(function* () {
			active_file = file;
			activity_message = `Selected ${file} fixture tab`;
		});

	const ChooseMenuAction = (label: string) =>
		Effect.gen(function* () {
			activity_message = `${label} is a local fixture action`;
			yield* CloseMenu;
		});

	const ResolvePermission = (result: Exclude<PermissionResult, "pending">) =>
		Effect.gen(function* () {
			permission_result = result;
			activity_message = `Permission fixture ${result}`;
		});

	const ResetPermission = Effect.gen(function* () {
		permission_result = "pending";
		activity_message = "Permission fixture reset";
	});

	const HandleKeydown = (pressed_key: string) =>
		Effect.gen(function* () {
			if (pressed_key === "Escape" && menu_open) {
				yield* CloseMenu;
			}
		});
</script>

<svelte:head>
	<title>Visual fixtures · Artisan</title>
	<meta name="description" content="Artisan interface fixture laboratory" />
</svelte:head>

<svelte:window onkeydown={yield* HandleKeydown(event.key)} />

<div
	class="fixture-viewport"
	class:light={fixture_theme === "light"}
	data-contrast={high_contrast ? "high" : "standard"}
	data-motion={reduced_motion ? "reduced" : "full"}
	data-zoom={zoomed ? "200" : "100"}
	data-labels={long_labels ? "long" : "standard"}
>
	<header class="fixture-toolbar">
		<div class="fixture-heading">
			<div class="fixture-kicker"><span class="fixture-dot"></span> Visual fixture</div>
			<h1>Artisan interface laboratory</h1>
			<p>Renderer-only specimens. No command below touches a project or backend.</p>
		</div>

		<div class="stress-controls" aria-label="Fixture stress controls">
			<div class="segmented-control" role="group" aria-label="Fixture theme">
				<button type="button" aria-pressed={fixture_theme === "dark"} onclick={yield* SetTheme("dark")}>
					<Moon size={15} aria-hidden="true" /> Dark
				</button>
				<button type="button" aria-pressed={fixture_theme === "light"} onclick={yield* SetTheme("light")}>
					<Sun size={15} aria-hidden="true" /> Light
				</button>
			</div>
			<button class="stress-toggle" type="button" aria-pressed={high_contrast} onclick={yield* ToggleContrast}>High contrast</button>
			<button class="stress-toggle" type="button" aria-pressed={reduced_motion} onclick={yield* ToggleMotion}>Reduced motion</button>
			<button class="stress-toggle" type="button" aria-pressed={zoomed} onclick={yield* ToggleZoom}>200% interface scale (simulated)</button>
			<button class="stress-toggle" type="button" aria-pressed={long_labels} onclick={yield* ToggleLabels}>Long labels</button>
		</div>
	</header>

	<div class="fixture-notice" role="status">
		<InfoCircle size={16} aria-hidden="true" />
		<span>{activity_message}</span>
		<span class="fixture-badge neutral">Local preview</span>
	</div>

	<main class="specimen-grid">
		<section class="specimen typography-specimen" aria-labelledby="typography-heading">
			<div class="specimen-header">
				<div>
					<p class="eyebrow">Foundation</p>
					<h2 id="typography-heading">Typography</h2>
				</div>
				<span class="fixture-badge fixture">Fixture</span>
			</div>

			<div class="type-scale">
				<p class="display-type">Make good work visible.</p>
				<h3>Workspace changes need a clear owner</h3>
				<p class="prose-type">Artisan keeps one visible checkout, records who changed each file, and gives the user a reviewable path back.</p>
				<p class="small-type">Secondary copy · Updated 12 seconds ago</p>
				<code>yield* WorkspaceFiles.replace(request)</code>
			</div>

			<div class="type-meta">
				<span><strong>Heading</strong> Artisan Neo 640</span>
				<span><strong>Text</strong> Artisan Neo 420</span>
				<span><strong>Code</strong> JetBrains Mono 400</span>
			</div>
		</section>

		<section class="specimen controls-specimen" aria-labelledby="controls-heading">
			<div class="specimen-header">
				<div>
					<p class="eyebrow">Actions</p>
					<h2 id="controls-heading">Controls</h2>
				</div>
			</div>

			<div class="button-row">
				<Button class="button primary" onclick={yield* ChooseMenuAction("Started run")}>
					<PlayerPlay size={15} aria-hidden="true" /> Start run
				</Button>
				<Button variant="secondary" class="button secondary" onclick={yield* ChooseMenuAction("Opened settings")}>
					<Settings size={15} aria-hidden="true" /> Settings
				</Button>
				<Button variant="ghost" class="button quiet" onclick={yield* ChooseMenuAction("Dismissed notice")}>
					<X size={15} aria-hidden="true" /> Dismiss
				</Button>
				<Button variant="destructive" class="button danger" onclick={yield* ChooseMenuAction("Stopped fixture")}>
					Stop fixture
				</Button>
				<Button variant="secondary" class="button secondary" disabled title="Workspace backend integration is unavailable">Commit unavailable</Button>
			</div>

			<div class="field-grid">
				<label class="field">
					<span>Search files</span>
					<span class="input-shell"><Search size={15} aria-hidden="true" /><input value="workspace change" aria-label="Search files fixture" /></span>
				</label>
				<label class="field">
					<span>Branch name</span>
					<span class="input-shell invalid"><GitBranch size={15} aria-hidden="true" /><input value="feature/shared-checkout" aria-invalid="true" aria-describedby="branch-error" /></span>
					<small id="branch-error">A worker already owns this mutation claim.</small>
				</label>
			</div>

			<div class="floating-specimens">
				<div class="menu-wrap">
					<button bind:this={menu_trigger} class="icon-button" type="button" aria-label="Open fixture actions" aria-expanded={menu_open} aria-controls="fixture-menu" onclick={yield* ToggleMenu}>
						<Dots size={17} aria-hidden="true" />
					</button>
					{#if menu_open}
						<div class="menu" id="fixture-menu" aria-label="Fixture thread actions">
							<p>Thread actions <span>Local preview</span></p>
							<button type="button" onclick={yield* ChooseMenuAction("Renamed thread")}>Rename fixture thread</button>
							<button type="button" onclick={yield* ChooseMenuAction("Pinned thread")}>Pin fixture thread</button>
							<button type="button" disabled>Archive unavailable</button>
						</div>
					{/if}
				</div>

				<div class="tooltip-wrap">
					<button class="icon-button tooltip-trigger" type="button" aria-describedby="fixture-tooltip">
						<Command size={17} aria-hidden="true" />
					</button>
					<span class="tooltip" id="fixture-tooltip" role="tooltip">Open command palette <kbd>Ctrl K</kbd></span>
				</div>
			</div>
		</section>

		<section class="specimen navigation-specimen" aria-labelledby="navigation-heading">
			<div class="specimen-header">
				<div>
					<p class="eyebrow">Navigation</p>
					<h2 id="navigation-heading">Modes and files</h2>
				</div>
				<span class="fixture-badge live">Local UI</span>
			</div>

			<div class="navigation-stack">
				<div>
					<p class="control-label">Workspace mode</p>
					<div class="mode-tabs" role="group" aria-label="Fixture workspace mode">
						<button type="button" aria-pressed={active_mode === "editor"} onclick={yield* SelectMode("editor")}><File size={15} aria-hidden="true" /> Editor</button>
						<button type="button" aria-pressed={active_mode === "chat"} onclick={yield* SelectMode("chat")}><Message size={15} aria-hidden="true" /> Chat</button>
						<button type="button" aria-pressed={active_mode === "orchestrator"} onclick={yield* SelectMode("orchestrator")}><Sparkles size={15} aria-hidden="true" /> Orchestrator</button>
					</div>
				</div>

				<div>
					<p class="control-label">Open files</p>
					<div class="file-tabs" role="group" aria-label="Fixture file tabs">
						<button type="button" aria-pressed={active_file === "agent"} onclick={yield* SelectFile("agent")}>AGENTS.md <span class="modified-dot" aria-label="Modified"></span></button>
						<button type="button" aria-pressed={active_file === "transport"} onclick={yield* SelectFile("transport")}>transport.ts</button>
						<button type="button" aria-pressed={active_file === "readme"} onclick={yield* SelectFile("readme")}>{long_labels ? "README-with-an-intentionally-unreasonably-long-project-label.md" : "README.md"}</button>
					</div>
				</div>
			</div>
		</section>

		<section class="specimen status-specimen" aria-labelledby="status-heading">
			<div class="specimen-header">
				<div>
					<p class="eyebrow">System state</p>
					<h2 id="status-heading">Badges and status rows</h2>
				</div>
			</div>

			<div class="badge-row" aria-label="Status badges">
				<span class="fixture-badge active">Running</span>
				<span class="fixture-badge waiting">Waiting</span>
				<span class="fixture-badge success">Complete</span>
				<span class="fixture-badge failure">Failed</span>
				<span class="fixture-badge neutral">Stopped</span>
			</div>

			<div class="status-list">
				<div class="status-row"><span class="status-icon active"><CircleFilled size={8} aria-hidden="true" /></span><span><strong>Juniper</strong><small>Editing workspace-file-service.ts</small></span><time>12s</time></div>
				<div class="status-row"><span class="status-icon waiting"><CircleFilled size={8} aria-hidden="true" /></span><span><strong>Solstice</strong><small>Waiting for mutation authority</small></span><time>1m</time></div>
				<div class="status-row"><span class="status-icon success"><Check size={14} aria-hidden="true" /></span><span><strong>Orbit</strong><small>Verified transport boundary</small></span><time>4m</time></div>
			</div>
		</section>

		<section class="specimen feedback-specimen" aria-labelledby="feedback-heading">
			<div class="specimen-header">
				<div>
					<p class="eyebrow">Feedback</p>
					<h2 id="feedback-heading">Loading and empty states</h2>
				</div>
			</div>

			<div class="feedback-grid">
				<div class="empty-state">
					<div class="empty-icon"><Search size={20} aria-hidden="true" /></div>
					<strong>No matching changes</strong>
					<p>Adjust the filter or wait for an attributed workspace change.</p>
					<Button variant="secondary" class="button secondary" onclick={yield* ChooseMenuAction("Cleared filter")}>Clear fixture filter</Button>
				</div>

				<div class="skeleton-card" aria-label="Loading fixture" aria-busy="true">
					<div class="skeleton avatar"></div>
					<div class="skeleton-copy"><div class="skeleton line wide"></div><div class="skeleton line"></div><div class="skeleton line short"></div></div>
				</div>
			</div>
		</section>

		<section class="specimen prompts-specimen" aria-labelledby="prompts-heading">
			<div class="specimen-header">
				<div>
					<p class="eyebrow">Attention</p>
					<h2 id="prompts-heading">Banners and permissions</h2>
				</div>
			</div>

			<div class="banner warning" role="status">
				<AlertTriangle size={17} aria-hidden="true" />
				<div><strong>Workspace conflict</strong><p>Another worker owns the visible checkout mutation claim.</p></div>
				<span class="fixture-badge waiting">Review</span>
			</div>

			<div class="permission-prompt">
				<div class="permission-icon"><Lock size={18} aria-hidden="true" /></div>
				<div class="permission-copy">
					<div><strong>Allow a fixture terminal command?</strong><span class="fixture-badge fixture">Fixture only</span></div>
					<code>pnpm --filter @artisan/frontend build</code>
					<p>This preview records a local visual state and executes nothing.</p>
				</div>
				<div class="permission-actions">
					{#if permission_result === "pending"}
						<Button variant="secondary" class="button secondary" onclick={yield* ResolvePermission("denied")}>Deny preview</Button>
						<Button class="button primary" onclick={yield* ResolvePermission("approved")}>Approve preview</Button>
					{:else}
						<span class:approved={permission_result === "approved"} class:denied={permission_result === "denied"}>{permission_result}</span>
						<Button variant="ghost" class="button quiet" onclick={yield* ResetPermission}>Reset</Button>
					{/if}
				</div>
			</div>

			<div class="blocked-control">
				<LayoutSidebarRight size={17} aria-hidden="true" />
				<div><strong>Preview lifecycle</strong><span>Backend unavailable · control intentionally disabled</span></div>
				<Button variant="secondary" class="button secondary" disabled>Open preview unavailable</Button>
			</div>
		</section>

		<section class="specimen diff-specimen" aria-labelledby="diff-heading">
			<div class="specimen-header">
				<div>
					<p class="eyebrow">Change review</p>
					<h2 id="diff-heading">Inline diff</h2>
				</div>
				<span class="fixture-badge fixture">Preview data</span>
			</div>

			<div class="diff-header"><span>modules/frontend/src/app.ts</span><span><b>+2</b> <i>−2</i></span></div>
			<div class="inline-diff" role="table" aria-label="Fixture inline diff">
				<div class="diff-line context" role="row"><span>18</span><span>18</span><code>const workspace = yield* Workspace.current;</code></div>
				<div class="diff-line removed" role="row"><span>19</span><span></span><code>- const title = "Editor";</code></div>
				<div class="diff-line removed" role="row"><span>20</span><span></span><code>- const owner = "unknown";</code></div>
				<div class="diff-line added" role="row"><span></span><span>19</span><code>+ const title = workspace.title;</code></div>
				<div class="diff-line added" role="row"><span></span><span>20</span><code>+ const owner = workspace.claim.owner;</code></div>
				<div class="diff-line context" role="row"><span>21</span><span>21</span><code>return { title, owner };</code></div>
			</div>
		</section>

		<section class="specimen terminal-specimen" aria-labelledby="terminal-heading">
			<div class="specimen-header">
				<div>
					<p class="eyebrow">Process surface</p>
					<h2 id="terminal-heading">Terminal chrome</h2>
				</div>
				<span class="fixture-badge fixture">Static fixture</span>
			</div>

			<div class="terminal-window">
				<div class="terminal-bar">
					<div class="terminal-tabs"><button type="button" aria-pressed="true"><Terminal2 size={14} aria-hidden="true" /> frontend</button><button type="button" disabled><Plus size={14} aria-hidden="true" /> New unavailable</button></div>
					<div class="terminal-meta"><span class="terminal-ready"></span> exited 0 <button class="terminal-menu" type="button" aria-label="Terminal options unavailable" disabled><Dots size={15} aria-hidden="true" /></button></div>
				</div>
				<pre aria-label="Static terminal output"><span class="prompt">PS C:\artisan-editor&gt;</span> pnpm --filter @artisan/frontend build
<span class="muted">vite v8.1.4 building for production...</span>
<span class="success-text">✓ built in 1.84s</span>
<span class="cursor" aria-hidden="true"></span></pre>
			</div>
		</section>
	</main>
</div>

<style>
	.fixture-viewport {
		min-height: 100dvh;
		padding: 22px;
		background: var(--canvas);
		color: var(--text-primary);
		font-size: 14px;
		transition: background var(--duration-medium) var(--ease-smooth-out);
	}

	.fixture-viewport[data-zoom="200"] {
		min-height: 50dvh;
		zoom: 2;
	}

	.fixture-viewport[data-zoom="200"] .specimen-grid {
		grid-template-columns: minmax(0, 1fr);
	}

	.fixture-viewport[data-contrast="high"] {
		--text-primary: light-dark(oklch(0.08 0 0), oklch(1 0 0));
		--text-secondary: light-dark(oklch(0.18 0 0), oklch(0.93 0 0));
		--text-muted: light-dark(oklch(0.3 0 0), oklch(0.82 0 0));
		--line: light-dark(oklch(0.25 0 0), oklch(0.78 0 0));
		--line-strong: light-dark(oklch(0.08 0 0), oklch(1 0 0));
		--focus: oklch(0.78 0.2 240);
	}

	.fixture-viewport[data-motion="reduced"] *,
	.fixture-viewport[data-motion="reduced"] *::before,
	.fixture-viewport[data-motion="reduced"] *::after {
		animation: none !important;
		transition: none !important;
		scroll-behavior: auto !important;
	}

	.fixture-toolbar {
		display: flex;
		align-items: flex-end;
		justify-content: space-between;
		gap: 24px;
		max-width: 1480px;
		margin: 0 auto 16px;
	}

	.fixture-heading h1 {
		margin: 5px 0 2px;
		font-size: clamp(24px, 3vw, 38px);
		font-weight: 660;
		letter-spacing: -0.05em;
	}

	.fixture-heading p {
		margin: 0;
		color: var(--text-muted);
	}

	.fixture-kicker,
	.eyebrow,
	.control-label {
		margin: 0;
		color: var(--text-muted);
		font-size: 11px;
		font-weight: 650;
		letter-spacing: 0.08em;
		text-transform: uppercase;
	}

	.fixture-kicker {
		display: flex;
		align-items: center;
		gap: 7px;
	}

	.fixture-dot,
	.terminal-ready {
		width: 7px;
		height: 7px;
		border-radius: var(--radius-pill);
		background: var(--run-complete);
		box-shadow: var(--shadow-status);
	}

	.stress-controls,
	.button-row,
	.badge-row,
	.floating-specimens {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: 7px;
	}

	.segmented-control {
		display: flex;
		padding: 3px;
		border: 1px solid var(--line);
		border-radius: var(--radius-control);
		background: var(--pane-inset);
	}

	.segmented-control button,
	.stress-toggle {
		display: inline-flex;
		min-height: 28px;
		align-items: center;
		gap: 5px;
		padding: 4px 9px;
		border: 1px solid transparent;
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--text-secondary);
		font-size: 12px;
		cursor: pointer;
	}

	.segmented-control button[aria-pressed="true"],
	.stress-toggle[aria-pressed="true"] {
		border-color: var(--line);
		background: var(--raised);
		box-shadow: var(--shadow-xs);
		color: var(--text-primary);
	}

	.stress-toggle {
		border-color: var(--line);
		background: var(--pane);
	}

	.fixture-notice {
		display: flex;
		min-height: 36px;
		max-width: 1480px;
		align-items: center;
		gap: 8px;
		margin: 0 auto 12px;
		padding: 7px 10px;
		border: 1px solid var(--line);
		border-radius: var(--radius-control);
		background: var(--pane-inset);
		color: var(--text-secondary);
	}

	.fixture-notice > span:nth-child(2) {
		min-width: 0;
		flex: 1;
		overflow-wrap: anywhere;
	}

	.specimen-grid {
		display: grid;
		grid-template-columns: repeat(12, minmax(0, 1fr));
		gap: 12px;
		max-width: 1480px;
		margin: 0 auto;
	}

	.specimen {
		min-width: 0;
		padding: 16px;
		border: 1px solid var(--line);
		border-radius: var(--radius-pane);
		background: var(--pane);
		box-shadow: var(--shadow-xs);
	}

	.typography-specimen,
	.controls-specimen,
	.prompts-specimen,
	.diff-specimen,
	.terminal-specimen {
		grid-column: span 6;
	}

	.navigation-specimen,
	.status-specimen,
	.feedback-specimen {
		grid-column: span 4;
	}

	.specimen-header {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: 12px;
		margin-bottom: 15px;
	}

	.specimen-header h2 {
		margin: 2px 0 0;
		font-size: 18px;
		font-weight: 650;
		letter-spacing: -0.04em;
	}

	.type-scale {
		display: grid;
		gap: 10px;
	}

	.type-scale > * {
		margin: 0;
	}

	.display-type {
		font-family: var(--font-heading);
		font-size: clamp(28px, 4vw, 52px);
		font-weight: 690;
		line-height: 0.98;
		letter-spacing: -0.055em;
	}

	.type-scale h3 {
		font-size: 20px;
		font-weight: 620;
		letter-spacing: -0.04em;
	}

	.prose-type {
		max-width: 62ch;
		color: var(--text-secondary);
		font-size: 15px;
		line-height: 1.55;
		letter-spacing: -0.025em;
	}

	.small-type {
		color: var(--text-muted);
		font-size: 12px;
	}

	code,
	.diff-line,
	.terminal-window pre {
		font-family: var(--font-mono);
	}

	.type-scale code {
		width: fit-content;
		max-width: 100%;
		padding: 5px 7px;
		border: 1px solid var(--line);
		border-radius: var(--radius-sm);
		background: var(--pane-inset);
		color: var(--text-secondary);
		overflow-wrap: anywhere;
	}

	.type-meta {
		display: flex;
		flex-wrap: wrap;
		gap: 6px 14px;
		margin-top: 16px;
		padding-top: 12px;
		border-top: 1px solid var(--line);
		color: var(--text-muted);
		font-size: 11px;
	}

	.type-meta strong {
		color: var(--text-secondary);
	}

	:global(.button) {
		display: inline-flex;
		min-height: 31px;
		align-items: center;
		justify-content: center;
		gap: 6px;
		padding: 5px 10px;
		border: 1px solid transparent;
		border-radius: var(--radius-control);
		font-size: 12px;
		font-weight: 590;
		cursor: pointer;
		transition: background var(--duration-fast), border-color var(--duration-fast), transform var(--duration-fast);
	}

	:global(.button:active:not(:disabled)) {
		transform: translateY(1px);
	}

	:global(.button.primary) {
		background: var(--text-primary);
		color: var(--canvas);
	}

	:global(.button.secondary) {
		border-color: var(--line);
		background: var(--raised);
		color: var(--text-primary);
	}

	:global(.button.quiet) {
		background: transparent;
		color: var(--text-secondary);
	}

	:global(.button.danger) {
		border-color: color-mix(in oklch, var(--run-failed) 50%, var(--line));
		background: color-mix(in oklch, var(--run-failed) 14%, var(--raised));
		color: var(--run-failed);
	}

	button:disabled {
		cursor: not-allowed;
		opacity: 0.48;
	}

	.field-grid {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: 10px;
		margin-top: 14px;
	}

	.field {
		display: grid;
		gap: 5px;
		color: var(--text-secondary);
		font-size: 12px;
	}

	.input-shell {
		display: flex;
		min-width: 0;
		align-items: center;
		gap: 7px;
		padding: 0 9px;
		border: 1px solid var(--line-strong);
		border-radius: var(--radius-control);
		background: var(--pane-inset);
		color: var(--text-muted);
	}

	.input-shell:focus-within {
		border-color: var(--focus);
		box-shadow: var(--shadow-focus);
	}

	.input-shell.invalid {
		border-color: var(--conflict);
	}

	.input-shell input {
		min-width: 0;
		width: 100%;
		height: 32px;
		border: 0;
		outline: 0;
		background: transparent;
		color: var(--text-primary);
	}

	.field small {
		color: var(--conflict);
	}

	.floating-specimens {
		margin-top: 14px;
	}

	.menu-wrap,
	.tooltip-wrap {
		position: relative;
	}

	.icon-button,
	.terminal-menu {
		display: grid;
		width: 31px;
		height: 31px;
		place-items: center;
		border: 1px solid var(--line);
		border-radius: var(--radius-control);
		background: var(--raised);
		color: var(--text-secondary);
		cursor: pointer;
	}

	.menu {
		position: absolute;
		bottom: calc(100% + 6px);
		left: 0;
		z-index: 4;
		display: grid;
		width: min(220px, calc(100vw - 48px));
		padding: 5px;
		border: 1px solid var(--line-strong);
		border-radius: var(--radius-raised);
		background: var(--raised);
		box-shadow: var(--shadow-overlay);
	}

	.menu p {
		display: flex;
		justify-content: space-between;
		gap: 8px;
		margin: 0;
		padding: 6px 7px;
		border-bottom: 1px solid var(--line);
		color: var(--text-muted);
		font-size: 10px;
		text-transform: uppercase;
	}

	.menu p span {
		text-transform: none;
	}

	.menu button {
		padding: 7px;
		border: 0;
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--text-secondary);
		text-align: left;
		cursor: pointer;
	}

	.menu button:hover:not(:disabled) {
		background: var(--raised-hover);
		color: var(--text-primary);
	}

	.tooltip {
		position: absolute;
		bottom: calc(100% + 7px);
		left: 50%;
		z-index: 5;
		width: max-content;
		max-width: 230px;
		padding: 6px 8px;
		border: 1px solid var(--line-strong);
		border-radius: var(--radius-sm);
		background: var(--text-primary);
		color: var(--canvas);
		font-size: 11px;
		opacity: 0;
		pointer-events: none;
		transform: translate(-50%, 3px);
		transition: opacity var(--duration-fast), transform var(--duration-fast);
	}

	.tooltip-trigger:hover + .tooltip,
	.tooltip-trigger:focus-visible + .tooltip {
		opacity: 1;
		transform: translate(-50%, 0);
	}

	.tooltip kbd {
		margin-left: 5px;
		opacity: 0.7;
	}

	.navigation-stack {
		display: grid;
		gap: 16px;
	}

	.mode-tabs,
	.file-tabs {
		display: flex;
		min-width: 0;
		margin-top: 6px;
		overflow-x: auto;
	}

	.mode-tabs {
		gap: 3px;
		padding: 3px;
		border: 1px solid var(--line);
		border-radius: var(--radius-control);
		background: var(--pane-inset);
	}

	.mode-tabs button,
	.file-tabs button {
		display: inline-flex;
		min-width: max-content;
		min-height: 30px;
		align-items: center;
		justify-content: center;
		gap: 5px;
		border: 0;
		background: transparent;
		color: var(--text-muted);
		font-size: 12px;
		cursor: pointer;
	}

	.mode-tabs button {
		flex: 1;
		padding: 4px 7px;
		border-radius: var(--radius-sm);
	}

	.mode-tabs button[aria-pressed="true"] {
		background: var(--raised);
		box-shadow: var(--shadow-xs);
		color: var(--text-primary);
	}

	.file-tabs {
		border-bottom: 1px solid var(--line);
	}

	.file-tabs button {
		position: relative;
		padding: 4px 10px 7px;
		border-right: 1px solid var(--line);
	}

	.file-tabs button[aria-pressed="true"] {
		background: var(--pane-inset);
		color: var(--text-primary);
	}

	.file-tabs button[aria-pressed="true"]::after {
		position: absolute;
		right: 8px;
		bottom: -1px;
		left: 8px;
		height: 2px;
		background: var(--focus);
		content: "";
	}

	.modified-dot {
		width: 5px;
		height: 5px;
		border-radius: var(--radius-pill);
		background: var(--permission);
	}

	.fixture-badge {
		display: inline-flex;
		min-height: 20px;
		align-items: center;
		width: fit-content;
		padding: 2px 7px;
		border: 1px solid var(--line);
		border-radius: var(--radius-pill);
		background: var(--pane-inset);
		color: var(--text-muted);
		font-size: 10px;
		font-weight: 650;
		letter-spacing: 0.02em;
		white-space: nowrap;
	}

	.fixture-badge.active,
	.status-icon.active {
		color: var(--run-active);
	}

	.fixture-badge.waiting,
	.status-icon.waiting {
		color: var(--run-waiting);
	}

	.fixture-badge.success,
	.status-icon.success,
	.success-text {
		color: var(--run-complete);
	}

	.fixture-badge.failure {
		color: var(--run-failed);
	}

	.fixture-badge.live {
		border-color: color-mix(in oklch, var(--run-active) 40%, var(--line));
		color: var(--run-active);
	}

	.fixture-badge.fixture {
		border-color: color-mix(in oklch, var(--permission) 40%, var(--line));
		color: var(--permission);
	}

	.status-list {
		display: grid;
		margin-top: 13px;
		border-top: 1px solid var(--line);
	}

	.status-row {
		display: grid;
		grid-template-columns: 18px minmax(0, 1fr) auto;
		align-items: center;
		gap: 7px;
		min-height: 45px;
		border-bottom: 1px solid var(--line);
	}

	.status-row > span:nth-child(2) {
		display: grid;
		min-width: 0;
	}

	.status-row strong {
		font-size: 12px;
		font-weight: 620;
	}

	.status-row small,
	.status-row time {
		color: var(--text-muted);
		font-size: 10px;
	}

	.status-row small {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.feedback-grid {
		display: grid;
		grid-template-columns: 1fr;
		gap: 10px;
	}

	.empty-state {
		display: grid;
		justify-items: center;
		padding: 18px 12px;
		border: 1px dashed var(--line-strong);
		border-radius: var(--radius-raised);
		background: var(--pane-inset);
		text-align: center;
	}

	.empty-icon {
		display: grid;
		width: 38px;
		height: 38px;
		place-items: center;
		margin-bottom: 8px;
		border: 1px solid var(--line);
		border-radius: var(--radius-raised);
		background: var(--raised);
		color: var(--text-muted);
	}

	.empty-state p {
		max-width: 32ch;
		margin: 4px 0 12px;
		color: var(--text-muted);
		font-size: 11px;
	}

	.skeleton-card {
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 12px;
		border: 1px solid var(--line);
		border-radius: var(--radius-raised);
		background: var(--pane-inset);
	}

	.skeleton {
		background: linear-gradient(90deg, var(--raised) 20%, var(--raised-hover) 48%, var(--raised) 76%);
		background-size: 220% 100%;
		animation: skeleton-shift var(--duration-ambient) ease-in-out infinite;
	}

	.skeleton.avatar {
		width: 34px;
		height: 34px;
		flex: none;
		border-radius: var(--radius-raised);
	}

	.skeleton-copy {
		display: grid;
		width: 100%;
		gap: 6px;
	}

	.skeleton.line {
		width: 66%;
		height: 6px;
		border-radius: var(--radius-pill);
	}

	.skeleton.line.wide {
		width: 92%;
	}

	.skeleton.line.short {
		width: 38%;
	}

	@keyframes skeleton-shift {
		to { background-position: -220% 0; }
	}

	.banner,
	.permission-prompt,
	.blocked-control {
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 10px;
		border: 1px solid var(--line);
		border-radius: var(--radius-raised);
		background: var(--pane-inset);
	}

	.banner.warning {
		border-color: color-mix(in oklch, var(--conflict) 42%, var(--line));
		background: color-mix(in oklch, var(--conflict-surface) 58%, var(--pane-inset));
		color: var(--conflict);
	}

	.banner > div,
	.blocked-control > div {
		display: grid;
		min-width: 0;
		flex: 1;
	}

	.banner p {
		margin: 2px 0 0;
		color: var(--text-secondary);
		font-size: 11px;
	}

	.permission-prompt {
		align-items: flex-start;
		margin-top: 9px;
		border-color: color-mix(in oklch, var(--permission) 42%, var(--line));
		background: color-mix(in oklch, var(--permission-surface) 45%, var(--pane-inset));
	}

	.permission-icon {
		display: grid;
		width: 34px;
		height: 34px;
		flex: none;
		place-items: center;
		border-radius: var(--radius-raised);
		background: var(--permission-surface);
		color: var(--permission);
	}

	.permission-copy {
		display: grid;
		min-width: 0;
		flex: 1;
		gap: 5px;
	}

	.permission-copy > div {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: 7px;
	}

	.permission-copy code {
		width: fit-content;
		max-width: 100%;
		padding: 4px 6px;
		border-radius: var(--radius-sm);
		background: var(--pane-inset);
		color: var(--text-secondary);
		overflow-wrap: anywhere;
	}

	.permission-copy p {
		margin: 0;
		color: var(--text-muted);
		font-size: 11px;
	}

	.permission-actions {
		display: flex;
		align-items: center;
		gap: 5px;
	}

	.permission-actions .approved {
		color: var(--run-complete);
	}

	.permission-actions .denied {
		color: var(--run-failed);
	}

	.blocked-control {
		margin-top: 9px;
		color: var(--text-muted);
	}

	.blocked-control span {
		font-size: 11px;
	}

	.diff-header {
		display: flex;
		justify-content: space-between;
		gap: 12px;
		padding: 8px 10px;
		border: 1px solid var(--line);
		border-bottom: 0;
		border-radius: var(--radius-raised) var(--radius-raised) 0 0;
		background: var(--pane-inset);
		color: var(--text-secondary);
		font-family: var(--font-mono);
		font-size: 11px;
	}

	.diff-header b {
		color: var(--diff-added);
	}

	.diff-header i {
		margin-left: 6px;
		color: var(--diff-removed);
		font-style: normal;
	}

	.inline-diff {
		border: 1px solid var(--line);
		border-radius: 0 0 var(--radius-raised) var(--radius-raised);
		overflow-x: auto;
	}

	.diff-line {
		display: grid;
		grid-template-columns: 34px 34px minmax(max-content, 1fr);
		min-width: 520px;
		font-size: 11px;
		line-height: 23px;
	}

	.diff-line > span {
		border-right: 1px solid var(--line);
		color: var(--text-muted);
		text-align: right;
		padding-right: 7px;
		user-select: none;
	}

	.diff-line code {
		padding: 0 10px;
		white-space: pre;
	}

	.diff-line.removed {
		background: var(--diff-removed-surface);
	}

	.diff-line.removed code {
		color: var(--diff-removed);
	}

	.diff-line.added {
		background: var(--diff-added-surface);
	}

	.diff-line.added code {
		color: var(--diff-added);
	}

	.terminal-window {
		border: 1px solid var(--line-strong);
		border-radius: var(--radius-raised);
		background: var(--pane-inset);
		overflow: hidden;
	}

	.terminal-bar {
		display: flex;
		min-height: 34px;
		align-items: center;
		justify-content: space-between;
		gap: 8px;
		border-bottom: 1px solid var(--line);
		background: var(--raised);
	}

	.terminal-tabs,
	.terminal-meta {
		display: flex;
		align-items: center;
	}

	.terminal-tabs button {
		display: inline-flex;
		min-height: 34px;
		align-items: center;
		gap: 5px;
		padding: 0 10px;
		border: 0;
		border-right: 1px solid var(--line);
		background: transparent;
		color: var(--text-muted);
		font-size: 11px;
	}

	.terminal-tabs button[aria-pressed="true"] {
		background: var(--pane-inset);
		color: var(--text-primary);
	}

	.terminal-meta {
		gap: 6px;
		padding-right: 5px;
		color: var(--text-muted);
		font-size: 10px;
	}

	.terminal-menu {
		width: 25px;
		height: 25px;
		border: 0;
		background: transparent;
	}

	.terminal-window pre {
		min-height: 122px;
		margin: 0;
		padding: 13px;
		color: var(--text-secondary);
		font-size: 11px;
		line-height: 1.75;
		overflow: auto;
	}

	.prompt {
		color: var(--run-active);
	}

	.muted {
		color: var(--text-muted);
	}

	.cursor {
		display: inline-block;
		width: 6px;
		height: 13px;
		margin-left: 2px;
		background: var(--text-secondary);
		vertical-align: -2px;
		animation: cursor-blink var(--duration-ambient) steps(1) infinite;
	}

	@keyframes cursor-blink {
		50% { opacity: 0; }
	}

	button:hover:not(:disabled),
	.icon-button:hover:not(:disabled) {
		border-color: var(--line-strong);
	}

	@media (max-width: 1040px) {
		.typography-specimen,
		.controls-specimen,
		.prompts-specimen,
		.diff-specimen,
		.terminal-specimen,
		.navigation-specimen,
		.status-specimen,
		.feedback-specimen {
			grid-column: span 12;
		}

		.fixture-toolbar {
			align-items: flex-start;
			flex-direction: column;
		}
	}

	@media (max-width: 640px) {
		.fixture-viewport {
			padding: 10px;
		}

		.field-grid {
			grid-template-columns: 1fr;
		}

		.permission-prompt,
		.blocked-control {
			align-items: stretch;
			flex-direction: column;
		}

		.permission-actions {
			justify-content: flex-end;
		}
	}

	@media (prefers-reduced-motion: reduce) {
		.fixture-viewport *,
		.fixture-viewport *::before,
		.fixture-viewport *::after {
			animation: none !important;
			transition: none !important;
		}
	}
</style>
