<script lang="ts" effect>
	import { Effect } from "effect";
	import {
		IconArchive as Archive,
		IconBrandDatabricks as BrandDatabricks,
		IconChevronDown as ChevronDown,
		IconLayoutGrid as LayoutGrid,
		IconLayoutSidebarLeftCollapse as CollapseLeft,
		IconMessagePlus as MessagePlus,
		IconPin as Pin,
	} from "@tabler/icons-svelte";

	import { thread_fixtures } from "./editor-fixtures";
	import { Button } from "$lib/components/ui/button";

	let {
		compact = false,
		instance_id,
		selected_thread,
		draft_threads,
		on_select_thread,
		on_new_chat,
		on_collapse,
	}: {
		compact?: boolean;
		instance_id: string;
		selected_thread: string;
		draft_threads: number;
		on_select_thread: (thread_id: string) => Effect.Effect<void>;
		on_new_chat: Effect.Effect<void>;
		on_collapse?: Effect.Effect<void>;
	} = $props();
	const threads_title_id = $derived(`${instance_id}-threads-title`);

	const SelectThread = (thread_id: string) =>
		Effect.gen(function* () {
			yield* on_select_thread(thread_id);
		});

	const NewChat = Effect.gen(function* () {
		yield* on_new_chat;
	});

	const CollapsePane = Effect.gen(function* () {
		if (on_collapse !== undefined) {
			yield* on_collapse;
		}
	});
</script>

<aside class:compact class="left-pane" aria-label="Thread navigation">
	<header class="brand-row">
		<span class="brand-mark"><BrandDatabricks size={19} stroke={1.7} aria-hidden="true" /></span>
		{#if !compact}<span class="brand-name">Artisan</span><span class="fixture-tag">Fixture</span>{/if}
		{#if on_collapse}
			<Button variant="ghost" size="icon-sm" class="collapse-pane" aria-label="Collapse thread navigation" title="Collapse thread navigation" onclick={yield* CollapsePane}>
				<CollapseLeft size={17} stroke={1.7} aria-hidden="true" />
			</Button>
		{/if}
	</header>

	<nav class="primary-actions" aria-label="Workspace actions">
		<Button variant="outline" class="action primary" aria-label="New chat" onclick={yield* NewChat}>
			<MessagePlus size={17} stroke={1.7} aria-hidden="true" />
			{#if !compact}<span>New chat{draft_threads > 0 ? ` (${draft_threads})` : ""}</span>{/if}
		</Button>
		<Button variant="ghost" class="action" aria-label="Marketplace unavailable" title="Marketplace contracts are not available yet" disabled>
			<LayoutGrid size={17} stroke={1.7} aria-hidden="true" />
			{#if !compact}<span>Marketplace</span>{/if}
		</Button>
	</nav>

	{#if !compact}
		<section class="thread-region" aria-labelledby={threads_title_id}>
			<div class="section-heading">
				<span id={threads_title_id}>Recent chats</span>
				<Archive size={14} stroke={1.7} aria-hidden="true" />
			</div>
			<div class="thread-list">
				{#each thread_fixtures as thread}
					<Button
						variant="ghost"
						class:active={selected_thread === thread.id}
						class="thread-row"
						type="button"
						onclick={yield* SelectThread(thread.id)}
					>
						<span class="thread-title">{thread.title}</span>
						<span class="thread-meta">{thread.meta}</span>
					</Button>
				{/each}
			</div>
		</section>
	{/if}

	<footer class="user-card">
		<div class="avatar" aria-hidden="true">SS</div>
		{#if !compact}
			<div class="user-copy"><strong>Sander</strong><span>Local workspace</span></div>
			<Pin size={14} stroke={1.7} aria-label="Pinned user" />
			<ChevronDown size={14} stroke={1.7} aria-hidden="true" />
		{/if}
	</footer>
</aside>

<style>
	.left-pane {
		display: flex;
		flex-direction: column;
		height: 100%;
		min-height: 0;
		border: 1px solid var(--line);
		border-radius: var(--radius-lg);
		background: var(--pane);
		overflow: hidden;
	}

	.brand-row {
		display: flex;
		align-items: center;
		gap: 9px;
		height: 48px;
		padding: 0 13px;
		border-bottom: 1px solid var(--line);
	}

	.brand-mark {
		display: grid;
		width: 25px;
		height: 25px;
		place-items: center;
		border-radius: var(--radius-sm);
		background: var(--text-primary);
		color: var(--canvas);
	}

	.brand-name {
		font-weight: 700;
		letter-spacing: -0.035em;
	}

	.fixture-tag {
		margin-left: auto;
		color: var(--text-muted);
		font-size: 10px;
		text-transform: uppercase;
		letter-spacing: 0.08em;
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

	.primary-actions {
		display: grid;
		gap: 4px;
		padding: 10px;
	}

	.action {
		display: flex;
		align-items: center;
		gap: 9px;
		min-height: 34px;
		padding: 0 9px;
		border: 1px solid transparent;
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--text-secondary);
		font: inherit;
		font-size: 13px;
		text-decoration: none;
		cursor: pointer;
	}

	.action:hover {
		background: var(--pane-inset);
		color: var(--text-primary);
	}

	.action.primary {
		border-color: var(--line);
		background: var(--raised);
		color: var(--text-primary);
	}

	.action:disabled {
		cursor: not-allowed;
		opacity: 0.48;
	}

	.thread-region {
		display: flex;
		flex: 1;
		min-height: 0;
		flex-direction: column;
		padding: 4px 10px 10px;
	}

	.section-heading {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 7px 8px;
		color: var(--text-muted);
		font-size: 10px;
		font-weight: 650;
		text-transform: uppercase;
		letter-spacing: 0.08em;
	}

	.thread-list {
		display: grid;
		gap: 2px;
		overflow-y: auto;
		overscroll-behavior: contain;
	}

	.thread-row {
		display: grid;
		gap: 3px;
		width: 100%;
		padding: 8px;
		border: 0;
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--text-secondary);
		font: inherit;
		text-align: left;
		cursor: pointer;
	}

	.thread-row:hover,
	.thread-row.active {
		background: var(--selection);
		color: var(--text-primary);
	}

	.thread-title {
		overflow: hidden;
		font-size: 12px;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.thread-meta {
		color: var(--text-muted);
		font-size: 10px;
	}

	.user-card {
		display: flex;
		align-items: center;
		gap: 9px;
		min-height: 52px;
		padding: 8px 10px;
		border-top: 1px solid var(--line);
		background: var(--pane-inset);
	}

	.avatar {
		display: grid;
		width: 30px;
		height: 30px;
		flex: 0 0 auto;
		place-items: center;
		border: 1px solid var(--line-strong);
		border-radius: 50%;
		background: var(--raised);
		font-size: 10px;
		font-weight: 700;
	}

	.user-copy {
		display: grid;
		flex: 1;
		font-size: 11px;
	}

	.user-copy span {
		color: var(--text-muted);
		font-size: 10px;
	}

	.compact .brand-row,
	.compact .primary-actions,
	.compact .user-card {
		justify-content: center;
		padding-inline: 6px;
	}

	.compact .primary-actions {
		display: flex;
		flex-direction: column;
	}

	.compact .action {
		justify-content: center;
		width: 36px;
		padding: 0;
	}

	.action:focus-visible,
	.thread-row:focus-visible,
	.collapse-pane:focus-visible {
		outline: 2px solid var(--focus);
		outline-offset: -2px;
	}

	@media (max-width: 1279px) {
		.collapse-pane {
			display: none;
		}
	}
</style>
