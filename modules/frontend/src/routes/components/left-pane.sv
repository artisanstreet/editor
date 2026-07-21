<script lang="ts" effect>
	import { Effect } from "effect";
	import type { DesktopIdentity } from "@artisan/transport/client";
	import {
		IconArchive as Archive,
		IconBrandDatabricks as BrandDatabricks,
		IconChevronDown as ChevronDown,
		IconLayoutGrid as LayoutGrid,
		IconLayoutSidebarLeftCollapse as CollapseLeft,
		IconMessagePlus as MessagePlus,
		IconDots as More,
		IconPencil as Pencil,
		IconPin as Pin,
		IconPinnedOff as Unpin,
	} from "@tabler/icons-svelte";

	import type { LiveWorkspaceActions, LiveWorkspaceSnapshot } from "$lib/live-workspace/store";
	import { Button } from "$lib/components/ui/button";
	import {
		Dialog,
		DialogContent,
		DialogDescription,
		DialogFooter,
		DialogHeader,
		DialogTitle,
	} from "$lib/components/ui/dialog";
	import {
		DropdownMenu,
		DropdownMenuContent,
		DropdownMenuItem,
		DropdownMenuTrigger,
	} from "$lib/components/ui/dropdown-menu";
	import { Input } from "$lib/components/ui/input";
	import MarketplaceDialog, { type MarketplaceApi } from "./marketplace-dialog.sv";

	let {
		compact = false,
		instance_id,
		live_snapshot,
		identity,
		actions,
		marketplace_api,
		on_select_thread,
		on_new_chat,
		on_collapse,
	}: {
		compact?: boolean;
		instance_id: string;
		live_snapshot: LiveWorkspaceSnapshot;
		identity: DesktopIdentity;
		actions: Pick<LiveWorkspaceActions, "Command">;
		marketplace_api?: MarketplaceApi;
		on_select_thread: (thread_id: string) => Effect.Effect<void>;
		on_new_chat: Effect.Effect<void>;
		on_collapse?: Effect.Effect<void>;
	} = $props();
	let marketplace_open = $state(false);
	let rename_open = $state(false);
	let rename_thread_id = $state("");
	let rename_title = $state("");
	let action_error = $state<string>();
	const threads_title_id = $derived(`${instance_id}-threads-title`);
	const identity_initials = $derived(
		identity.display_name
			.split(/\s+/u)
			.filter(Boolean)
			.slice(0, 2)
			.map((part) => part[0]?.toUpperCase() ?? "")
			.join("") || "AE",
	);
	const identity_hue = $derived(
		[...identity.avatar_seed].reduce(
			(hash, character) => (hash * 31 + (character.codePointAt(0) ?? 0)) % 360,
			0,
		),
	);

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

	const OpenMarketplace = Effect.gen(function* () {
		marketplace_open = true;
		if (marketplace_api !== undefined) {
			yield* marketplace_api.RefreshMarketplace({}).pipe(Effect.forkDetach);
		}
	});

	const ThreadCommand = (
		thread_id: string,
		payload: Parameters<LiveWorkspaceActions["Command"]>[0]["payload"],
	) =>
		actions.Command({ thread_id, payload }).pipe(
			Effect.matchEffect({
				onFailure: (error) => Effect.sync(() => (action_error = error.message)),
				onSuccess: () => Effect.sync(() => (action_error = undefined)),
			}),
		);
	const OpenRename = (thread_id: string, title: string) =>
		Effect.sync(() => {
			rename_thread_id = thread_id;
			rename_title = title;
			rename_open = true;
		});
	const RenameThread = Effect.gen(function* () {
		const title = rename_title.trim();
		if (title.length === 0 || rename_thread_id.length === 0) return;
		yield* ThreadCommand(rename_thread_id, { type: "thread.rename", title });
		if (action_error === undefined) rename_open = false;
	});
</script>

<aside class:compact class="left-pane" aria-label="Thread navigation">
	<header class="brand-row">
		<span class="brand-mark"><BrandDatabricks size={19} stroke={1.7} aria-hidden="true" /></span>
		{#if !compact}<span class="brand-name">Artisan Editor</span><span class="live-badge">Live</span>{/if}
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
		<Button variant="ghost" class="w-full justify-start gap-2" aria-label="Open Marketplace" onclick={yield* OpenMarketplace}>
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
					<div class={`group flex items-center rounded-md ${live_snapshot.selected_thread_id._tag === "Some" && live_snapshot.selected_thread_id.value === thread.thread_id ? "bg-muted text-foreground" : "text-muted-foreground"}`}>
						<Button variant="ghost" class="h-auto min-w-0 flex-1 justify-start px-2 py-2 text-left" type="button" onclick={yield* SelectThread(thread.thread_id)}>
							<span class="grid min-w-0 flex-1"><span class="truncate text-left text-sm">{thread.title}</span><span class="truncate text-left text-xs text-muted-foreground">{thread.live_status}{thread.primary_project ? ` · ${thread.primary_project.display_name}` : ""}</span></span>
							{#if thread.pinned}<Pin size={13} aria-label="Pinned thread" />{/if}
						</Button>
						<DropdownMenu>
							<DropdownMenuTrigger>
								{#snippet child({ props })}<Button {...props} variant="ghost" size="icon-xs" aria-label={`Actions for ${thread.title}`}><More size={14} /></Button>{/snippet}
							</DropdownMenuTrigger>
							<DropdownMenuContent align="end">
								<DropdownMenuItem onclick={yield* OpenRename(thread.thread_id, thread.title)}><Pencil size={14} />Rename</DropdownMenuItem>
								<DropdownMenuItem onclick={yield* ThreadCommand(thread.thread_id, { type: thread.pinned ? "thread.unpin" : "thread.pin" })}>{#if thread.pinned}<Unpin size={14} />Unpin{:else}<Pin size={14} />Pin{/if}</DropdownMenuItem>
								<DropdownMenuItem onclick={yield* ThreadCommand(thread.thread_id, { type: thread.archived_at === undefined ? "thread.archive" : "thread.restore" })}><Archive size={14} />{thread.archived_at === undefined ? "Archive" : "Restore"}</DropdownMenuItem>
								{#if thread.rehome_suggestion}<DropdownMenuItem onclick={yield* ThreadCommand(thread.thread_id, { type: "thread.project.assign", project: thread.rehome_suggestion.project })}>Move to {thread.rehome_suggestion.project.display_name}</DropdownMenuItem>{/if}
								{#if thread.project_locked}<DropdownMenuItem onclick={yield* ThreadCommand(thread.thread_id, { type: "thread.project.unlock", basis_affinity_version: thread.affinity_version })}>Use automatic project</DropdownMenuItem>{/if}
							</DropdownMenuContent>
						</DropdownMenu>
					</div>
				{/each}
				{/if}
				{#if action_error}<p class="p-2 text-xs text-destructive" role="alert">{action_error}</p>{/if}
			</div>
		</section>
	{/if}

	<footer class="user-card">
		<div class="avatar" style={`--avatar-hue:${identity_hue}`} aria-hidden="true">{#if identity.avatar_data_url}<img src={identity.avatar_data_url} alt="" />{:else}{identity_initials}{/if}</div>
		{#if !compact}
			<div class="user-copy"><strong>{identity.display_name}</strong><span>{identity.machine_name}</span></div>
			<DropdownMenu>
				<DropdownMenuTrigger>
					{#snippet child({ props })}
						<Button variant="ghost" size="icon-xs" aria-label="Open user actions" {...props}>
							<ChevronDown size={14} stroke={1.7} aria-hidden="true" />
						</Button>
					{/snippet}
				</DropdownMenuTrigger>
				<DropdownMenuContent align="end">
					<DropdownMenuItem disabled><Pin size={14} stroke={1.7} />{identity.machine_name}</DropdownMenuItem>
					<DropdownMenuItem disabled>Session settings are in the right pane</DropdownMenuItem>
				</DropdownMenuContent>
			</DropdownMenu>
		{/if}
	</footer>
</aside>

<MarketplaceDialog api={marketplace_api} live_snapshot={live_snapshot} bind:open={marketplace_open} />

<Dialog bind:open={rename_open}>
	<DialogContent>
		<DialogHeader><DialogTitle>Rename thread</DialogTitle><DialogDescription>Manual titles stay locked until you rename them again.</DialogDescription></DialogHeader>
		<Input bind:value={rename_title} aria-label="Thread title" onkeydown={event.key === "Enter" ? yield* RenameThread : undefined} />
		<DialogFooter><Button variant="outline" onclick={() => (rename_open = false)}>Cancel</Button><Button disabled={rename_title.trim().length === 0} onclick={yield* RenameThread}>Save title</Button></DialogFooter>
	</DialogContent>
</Dialog>

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
		background: linear-gradient(135deg, hsl(var(--avatar-hue) 72% 58%), hsl(calc(var(--avatar-hue) + 54) 68% 42%));
		color: white;
		font-size: 10px;
		font-weight: 700;
	}

	.avatar img {
		width: 100%;
		height: 100%;
		border-radius: inherit;
		object-fit: cover;
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
