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

	import type { LiveWorkspaceSnapshot } from "$lib/live-workspace/store";
	import { Button } from "$lib/components/ui/button";
	import {
		DropdownMenu,
		DropdownMenuContent,
		DropdownMenuItem,
		DropdownMenuTrigger,
	} from "$lib/components/ui/dropdown-menu";

	let {
		compact = false,
		instance_id,
		selected_thread,
		live_snapshot,
		on_select_thread,
		on_new_chat,
		on_collapse,
	}: {
		compact?: boolean;
		instance_id: string;
		selected_thread: string;
		live_snapshot: LiveWorkspaceSnapshot;
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
		{#if !compact}<span class="brand-name">Artisan</span><span class="live-badge">Live</span>{/if}
		{#if on_collapse}
			<Button variant="ghost" size="icon-sm" class="ml-auto text-muted-foreground" aria-label="Collapse thread navigation" title="Collapse thread navigation" onclick={yield* CollapsePane}>
				<CollapseLeft size={17} stroke={1.7} aria-hidden="true" />
			</Button>
		{/if}
	</header>

	<nav class="primary-actions" aria-label="Workspace actions">
		<Button variant="outline" class="w-full justify-start gap-2" aria-label="New chat" onclick={yield* NewChat}>
			<MessagePlus size={17} stroke={1.7} aria-hidden="true" />
			{#if !compact}<span>New chat</span>{/if}
		</Button>
		<Button variant="ghost" class="w-full justify-start gap-2" aria-label="Marketplace unavailable" title="Marketplace contracts are not available yet" disabled>
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
				{#if live_snapshot.threads.length === 0}
					<p class="empty-live-state">{live_snapshot.phase === "error" ? "Desktop session unavailable." : "No backend threads yet."}</p>
				{:else}
				{#each live_snapshot.threads as thread}
					<Button
						variant="ghost"
						class={`h-auto w-full justify-start rounded-md px-2 py-2 text-left ${live_snapshot.selected_thread_id._tag === "Some" && live_snapshot.selected_thread_id.value === thread.thread_id ? "bg-muted text-foreground" : "text-muted-foreground"}`}
						type="button"
						onclick={yield* SelectThread(thread.thread_id)}
					>
						<span class="w-full truncate text-left text-sm">{thread.title}</span>
						<span class="text-xs text-muted-foreground">{thread.live_status}</span>
					</Button>
				{/each}
				{/if}
			</div>
		</section>
	{/if}

	<footer class="user-card">
		<div class="avatar" aria-hidden="true">SS</div>
		{#if !compact}
			<div class="user-copy"><strong>Sander</strong><span>Local workspace</span></div>
			<DropdownMenu>
				<DropdownMenuTrigger>
					{#snippet child({ props })}
						<Button variant="ghost" size="icon-xs" aria-label="Open user actions" {...props}>
							<ChevronDown size={14} stroke={1.7} aria-hidden="true" />
						</Button>
					{/snippet}
				</DropdownMenuTrigger>
				<DropdownMenuContent align="end">
					<DropdownMenuItem disabled><Pin size={14} stroke={1.7} />Local workspace pinned</DropdownMenuItem>
					<DropdownMenuItem disabled>Settings are not available in this surface yet</DropdownMenuItem>
				</DropdownMenuContent>
			</DropdownMenu>
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

	.live-badge {
		margin-left: auto;
		color: var(--text-muted);
		font-size: 10px;
		text-transform: uppercase;
		letter-spacing: 0.08em;
	}

	.primary-actions {
		display: grid;
		gap: 4px;
		padding: 10px;
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

	.empty-live-state {
		margin: 8px;
		color: var(--text-muted);
		font-size: 11px;
		line-height: 1.45;
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


</style>
