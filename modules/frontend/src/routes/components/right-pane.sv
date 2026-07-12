<script lang="ts" effect>
	import { Effect } from "effect";
	import {
		IconActivity as Activity,
		IconBrandWindows as BrandWindows,
		IconGitBranch as GitBranch,
		IconLock as Lock,
		IconLayoutSidebarRightCollapse as CollapseRight,
		IconNetwork as Network,
		IconRobot as Robot,
		IconTerminal2 as Terminal2,
	} from "@tabler/icons-svelte";

	import { agent_fixtures, change_fixtures, permission_fixtures, session_fixture } from "./editor-fixtures";

	let { instance_id, on_collapse }: { instance_id: string; on_collapse?: Effect.Effect<void> } = $props();

	const CollapsePane = Effect.gen(function* () {
		if (on_collapse !== undefined) {
			yield* on_collapse;
		}
	});
</script>

<aside class="right-pane" aria-label="Session">
	<header class="session-header">
		<div><strong>Session</strong><span>Preview data</span></div>
		<span class="fixture-badge">Fixture</span>
		{#if on_collapse}
			<button class="collapse-pane" type="button" aria-label="Collapse session pane" title="Collapse session pane" onclick={yield* CollapsePane}>
				<CollapseRight size={17} stroke={1.7} aria-hidden="true" />
			</button>
		{/if}
	</header>

	<div class="session-scroll">
		<section class="session-card identity-card" aria-labelledby={`${instance_id}-session-title`}>
			<div class="section-title"><Activity size={14} stroke={1.7} aria-hidden="true" /><h2 id={`${instance_id}-session-title`}>Current session</h2><span class="status waiting">Waiting</span></div>
			<dl class="dense-list">
				<div><dt>Engine</dt><dd>{session_fixture.engine}</dd></div>
				<div><dt>Model</dt><dd>{session_fixture.model}</dd></div>
				<div><dt>Context</dt><dd>{session_fixture.context}</dd></div>
				<div><dt>Session ID</dt><dd class="mono">{session_fixture.id}</dd></div>
			</dl>
		</section>

		<section class="session-card" aria-labelledby={`${instance_id}-permissions-title`}>
			<div class="section-title"><Lock size={14} stroke={1.7} aria-hidden="true" /><h2 id={`${instance_id}-permissions-title`}>Permissions</h2></div>
			<ul class="row-list">
				{#each permission_fixtures as permission}
					<li><span>{permission.label}</span><strong class:permission={permission.tone === "permission"}>{permission.value}</strong></li>
				{/each}
			</ul>
		</section>

		<section class="session-card" aria-labelledby={`${instance_id}-changes-title`}>
			<div class="section-title"><GitBranch size={14} stroke={1.7} aria-hidden="true" /><h2 id={`${instance_id}-changes-title`}>Branch & changes</h2><span class="count">2</span></div>
			<div class="branch-row"><span>codex/backend-services</span><span>dirty</span></div>
			<ul class="change-list">
				{#each change_fixtures as change}
					<li><span class="change-path" title={change.path}>{change.path}</span><span class="diff-added">+{change.added}</span><span class="diff-removed">−{change.removed}</span></li>
				{/each}
			</ul>
		</section>

		<section class="session-card" aria-labelledby={`${instance_id}-terminal-title`}>
			<div class="section-title"><Terminal2 size={14} stroke={1.7} aria-hidden="true" /><h2 id={`${instance_id}-terminal-title`}>Terminals & ports</h2></div>
			<div class="terminal-preview">
				<div><BrandWindows size={13} stroke={1.7} aria-hidden="true" /><span>PowerShell</span><span class="status active">Running</span></div>
				<code>PS C:\artisan-editor&gt;</code>
			</div>
			<div class="empty-row"><Network size={13} stroke={1.7} aria-hidden="true" /><span>No preview ports in fixture</span></div>
		</section>

		<section class="session-card" aria-labelledby={`${instance_id}-agents-title`}>
			<div class="section-title"><Robot size={14} stroke={1.7} aria-hidden="true" /><h2 id={`${instance_id}-agents-title`}>Agents</h2><span class="count">{agent_fixtures.length}</span></div>
			<ul class="agent-list">
				{#each agent_fixtures as agent}
					<li><span class="agent-avatar">{agent.name.slice(0, 1)}</span><span><strong>{agent.name}</strong><small>{agent.role}</small></span><span class:active={agent.state === "Working"} class="agent-state">{agent.state}</span></li>
				{/each}
			</ul>
		</section>
	</div>
</aside>

<style>
	.right-pane {
		display: flex;
		flex-direction: column;
		height: 100%;
		min-height: 0;
		border: 1px solid var(--line);
		border-radius: var(--radius-lg);
		background: var(--pane);
		overflow: hidden;
	}

	.session-header {
		display: flex;
		min-height: 48px;
		align-items: center;
		justify-content: space-between;
		gap: 10px;
		padding: 0 12px;
		border-bottom: 1px solid var(--line);
	}

	.session-header > div {
		display: flex;
		align-items: baseline;
		gap: 8px;
	}

	.session-header strong {
		font-size: 12px;
	}

	.session-header span {
		color: var(--text-muted);
		font-size: 10px;
	}

	.fixture-badge {
		margin-left: auto;
		padding: 3px 6px;
		border: 1px solid var(--line);
		border-radius: var(--radius-sm);
		background: var(--pane-inset);
		text-transform: uppercase;
		letter-spacing: 0.07em;
	}

	.collapse-pane {
		display: grid;
		width: 28px;
		height: 28px;
		flex: 0 0 auto;
		place-items: center;
		border: 1px solid transparent;
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--text-muted);
		cursor: pointer;
	}

	.collapse-pane:hover {
		border-color: var(--line);
		background: var(--raised);
		color: var(--text-primary);
	}

	.collapse-pane:focus-visible {
		outline: 2px solid var(--focus);
		outline-offset: -2px;
	}

	.session-scroll {
		display: grid;
		align-content: start;
		gap: 0;
		padding: 6px 10px;
		overflow-y: auto;
		overscroll-behavior: contain;
	}

	.session-card {
		border: 0;
		border-bottom: 1px solid var(--line);
		border-radius: 0;
		background: transparent;
		overflow: hidden;
	}

	.session-card:last-child {
		border-bottom: 0;
	}

	.section-title {
		display: flex;
		min-height: 32px;
		align-items: center;
		gap: 7px;
		padding: 0 9px;
		border-bottom: 1px solid var(--line);
		color: var(--text-muted);
	}

	h2 {
		flex: 1;
		margin: 0;
		color: var(--text-secondary);
		font-size: 10px;
		font-weight: 650;
		text-transform: uppercase;
		letter-spacing: 0.06em;
	}

	.status,
	.count {
		padding: 2px 5px;
		border-radius: var(--radius-sm);
		background: var(--raised);
		color: var(--text-muted);
		font-size: 9px;
	}

	.status.waiting {
		color: var(--run-waiting);
	}

	.status.active,
	.agent-state.active {
		color: var(--run-active);
	}

	.dense-list {
		margin: 0;
		padding: 4px 9px;
	}

	.dense-list div,
	.row-list li {
		display: flex;
		min-height: 26px;
		align-items: center;
		justify-content: space-between;
		gap: 10px;
		border-bottom: 1px solid var(--line);
	}

	.dense-list div:last-child,
	.row-list li:last-child {
		border-bottom: 0;
	}

	dt,
	.row-list span {
		color: var(--text-muted);
		font-size: 10px;
	}

	dd,
	.row-list strong {
		margin: 0;
		color: var(--text-secondary);
		font-size: 10px;
		font-weight: 500;
		text-align: right;
	}

	.mono,
	.terminal-preview code {
		font-family: var(--font-mono);
	}

	.row-list,
	.change-list,
	.agent-list {
		margin: 0;
		padding: 4px 9px;
		list-style: none;
	}

	.row-list strong.permission {
		color: var(--permission);
	}

	.branch-row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 8px;
		padding: 8px 9px 5px;
		color: var(--text-secondary);
		font-size: 10px;
	}

	.branch-row span:last-child {
		color: var(--conflict);
	}

	.change-list {
		display: grid;
		gap: 4px;
		padding-bottom: 8px;
	}

	.change-list li {
		display: grid;
		grid-template-columns: minmax(0, 1fr) auto auto;
		gap: 6px;
		font-size: 9px;
	}

	.change-path {
		overflow: hidden;
		color: var(--text-muted);
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.diff-added {
		color: var(--diff-added);
	}

	.diff-removed {
		color: var(--diff-removed);
	}

	.terminal-preview {
		display: grid;
		gap: 7px;
		margin: 8px;
		padding: 8px;
		border: 1px solid var(--line);
		border-radius: var(--radius-sm);
		background: var(--canvas);
	}

	.terminal-preview > div {
		display: flex;
		align-items: center;
		gap: 6px;
		color: var(--text-secondary);
		font-size: 10px;
	}

	.terminal-preview .status {
		margin-left: auto;
	}

	.terminal-preview code {
		color: var(--text-muted);
		font-size: 9px;
	}

	.empty-row {
		display: flex;
		align-items: center;
		gap: 7px;
		padding: 0 9px 9px;
		color: var(--text-muted);
		font-size: 9px;
	}

	.agent-list {
		display: grid;
		gap: 5px;
		padding-block: 8px;
	}

	.agent-list li {
		display: grid;
		grid-template-columns: 24px minmax(0, 1fr) auto;
		align-items: center;
		gap: 7px;
	}

	.agent-avatar {
		display: grid;
		width: 24px;
		height: 24px;
		place-items: center;
		border: 1px solid var(--line-strong);
		border-radius: 50%;
		background: var(--raised);
		font-size: 9px;
		font-weight: 700;
	}

	.agent-list li > span:nth-child(2) {
		display: grid;
		gap: 1px;
	}

	.agent-list strong {
		font-size: 10px;
	}

	.agent-list small,
	.agent-state {
		color: var(--text-muted);
		font-size: 9px;
	}
</style>
