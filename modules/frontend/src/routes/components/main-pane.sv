<script lang="ts" effect>
	import { Effect, Option } from "effect";
	import {
		IconArrowUp as ArrowUp,
		IconFileCode as FileCode,
		IconGitMerge as GitMerge,
		IconMessage as Message,
	} from "@tabler/icons-svelte";

	import type { LiveWorkspaceSnapshot } from "$lib/live-workspace/store";
	import {
		CreateWorkspaceState,
		type CloseTabOutcome,
		type DirtyCloseConfirmation,
		type WorkspaceMode,
	} from "$lib/workspace/workspace-tab-model";
	import { Button } from "$lib/components/ui/button";
	import { Textarea } from "$lib/components/ui/textarea";

	import FileTabStrip from "./file-tab-strip.sv";
	import ModeSwitcher from "./mode-switcher.sv";
	import QuickOpen from "./quick-open.sv";
	import WorkspaceNavigation from "./workspace-navigation.sv";

	let {
		live_snapshot,
		on_send_live_message,
	}: {
		live_snapshot: LiveWorkspaceSnapshot;
		on_send_live_message: (text: string) => Effect.Effect<void>;
	} = $props();

	let mode = $state<WorkspaceMode>("editor");
	let chat_draft = $state("");
	let editor_viewport = $state<HTMLDivElement>();
	let editor_scroll_top = $state(0);
	const empty_workspace = yield* CreateWorkspaceState();

	const SelectMode = (next_mode: WorkspaceMode) =>
		Effect.sync(() => {
			mode = next_mode;
		});

	const SendLiveMessage = (event?: KeyboardEvent) =>
		Effect.gen(function* () {
			if (
				chat_draft.trim().length === 0 ||
				Option.isNone(live_snapshot.thread_work)
			) {
				return;
			}

			event?.preventDefault();
			yield* on_send_live_message(chat_draft);
			if (Option.isNone(live_snapshot.error)) chat_draft = "";
		});

	const HandleComposerKey = (event: KeyboardEvent) =>
		Effect.gen(function* () {
			if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
				yield* SendLiveMessage(event);
			}
		});

	/** These callbacks preserve the tab interaction seam while backend file discovery is unavailable. */
	const Noop = Effect.void;
	const TabNotFound = (tab_id: string): Effect.Effect<CloseTabOutcome> =>
		Effect.succeed({ _tag: "TabNotFound", state: empty_workspace, tab_id });
	const ConfirmTabNotFound = (
		confirmation: DirtyCloseConfirmation,
	): Effect.Effect<CloseTabOutcome> => TabNotFound(confirmation.tab_id);
	const CaptureEditorScroll = (scroll_top: number) =>
		Effect.sync(() => {
			editor_scroll_top = scroll_top;
		});
</script>

<section class="main-pane" aria-label="Workspace">
	<header class="workspace-header">
		<div class="workspace-title">
			<strong>Workspace</strong>
			<span>{live_snapshot.phase}</span>
		</div>
		<div class="workspace-primary-controls">
			<QuickOpen files={empty_workspace.recent_files} on_open={() => Noop} />
			<ModeSwitcher {mode} on_select={SelectMode} />
		</div>
	</header>

	{#if mode === "editor"}
		<FileTabStrip
			visible_tabs={empty_workspace.tabs}
			overflow_tabs={[]}
			active_tab_id={undefined}
			on_activate={() => Noop}
			on_pin={() => Noop}
			on_promote={() => Noop}
			on_close={TabNotFound}
			on_confirm_close={ConfirmTabNotFound}
		/>
		<WorkspaceNavigation
			breadcrumbs={[]}
			recent_files={empty_workspace.recent_files}
			changed_files={empty_workspace.changed_files}
			overflow_tabs={[]}
			on_open_recent={() => Noop}
			on_open_changed={() => Noop}
			on_activate_overflow={() => Noop}
		/>
		<div bind:this={editor_viewport} class="mode-surface empty-surface editor-viewport" onscroll={yield* CaptureEditorScroll(editor_viewport?.scrollTop ?? editor_scroll_top)}>
			<FileCode size={22} aria-hidden="true" />
			<h1>Editor ready</h1>
			<p>File discovery and Monaco content will appear here when the authoritative workspace projection is connected.</p>
		</div>
	{:else if mode === "chat"}
		<div class="mode-surface chat-surface">
			<div class="live-copy">
				<Message size={22} aria-hidden="true" />
				<h1>Live conversation</h1>
				{#if Option.isSome(live_snapshot.thread_work)}
					<p>{live_snapshot.thread_work.value.display_name} is {live_snapshot.thread_work.value.status} with {live_snapshot.thread_work.value.engine_id}.</p>
				{:else}
					<p>Select an existing thread to message its active Codex run.</p>
				{/if}
			</div>
			{#if Option.isSome(live_snapshot.transcript)}
				<div class="transcript" aria-label="Authoritative transcript">
					{#if live_snapshot.transcript.value.entries.length === 0}<p>No transcript entries yet.</p>{/if}
					{#each live_snapshot.transcript.value.entries as entry (entry.event_id)}
						<article><small>{entry.payload.type}</small><p>{"text" in entry.payload ? entry.payload.text : "description" in entry.payload ? entry.payload.description : "assumption" in entry.payload ? entry.payload.assumption : "risk" in entry.payload ? `${entry.payload.risk}: ${entry.payload.resolution}` : "Recorded activity"}</p></article>
					{/each}
				</div>
			{:else}<p class="text-muted-foreground">Loading authoritative transcript…</p>{/if}
			<label class="chat-composer">
				<span>Message active Codex run</span>
				<Textarea bind:value={chat_draft} aria-label="Message active Codex run" onkeydown={yield* HandleComposerKey(event)} />
				<Button size="icon-xs" type="button" aria-label="Send message" disabled={chat_draft.trim().length === 0 || Option.isNone(live_snapshot.thread_work)} onclick={yield* SendLiveMessage()}><ArrowUp size={16} aria-hidden="true" /></Button>
			</label>
		</div>
	{:else}
		<div class="mode-surface chat-surface">
			<GitMerge size={22} aria-hidden="true" />
			<h1>Orchestrator</h1>
			{#if Option.isSome(live_snapshot.orchestration_groups)}
				{#if live_snapshot.orchestration_groups.value.groups.length === 0}<p>No orchestration groups for this thread.</p>{/if}
				<div class="group-list">{#each live_snapshot.orchestration_groups.value.groups as group (group.group_id)}<span class="rounded-md border px-2 py-1 text-xs">{group.state} · {group.group_id}</span>{/each}</div>
			{:else}<p>Loading groups…</p>{/if}
			{#if Option.isSome(live_snapshot.orchestration_graph)}<div class="transcript"><p>{live_snapshot.orchestration_graph.value.assignments.length} assignments · {live_snapshot.orchestration_graph.value.agent_runs.length} runs · {live_snapshot.orchestration_graph.value.edges.length} edges</p></div>{:else if Option.isSome(live_snapshot.selected_group_id)}<p>Loading selected graph…</p>{/if}
		</div>
	{/if}
</section>

<style>
	.main-pane {
		display: flex;
		height: 100%;
		min-height: 0;
		flex-direction: column;
		border: 1px solid var(--line);
		border-radius: var(--radius-lg);
		background: var(--pane);
		overflow: hidden;
	}

	.workspace-header {
		display: flex;
		min-height: 48px;
		align-items: center;
		justify-content: space-between;
		gap: 12px;
		padding: 0 var(--pane-action-space, 10px) 0 14px;
		border-bottom: 1px solid var(--line);
	}

	.workspace-primary-controls {
		display: flex;
		align-items: center;
		gap: 8px;
	}

	.workspace-title {
		display: flex;
		min-width: 0;
		align-items: baseline;
		gap: 8px;
	}

	.workspace-title strong {
		font-size: 12px;
	}

	.workspace-title span,
	.chat-composer {
		color: var(--text-muted);
		font-size: 10px;
	}

	.mode-surface {
		display: grid;
		min-height: 0;
		flex: 1;
	}

	.empty-surface {
		place-content: center;
		justify-items: center;
		gap: 8px;
		padding: 24px;
		text-align: center;
	}

	.editor-viewport {
		overflow: auto;
		outline: none;
	}

	.empty-surface h1,
	.live-copy h1 {
		margin: 0;
		font-size: 18px;
	}

	.empty-surface p,
	.live-copy p {
		max-width: 460px;
		margin: 0;
		color: var(--text-muted);
		font-size: 12px;
		line-height: 1.5;
	}

	.chat-surface {
		align-content: center;
		gap: 24px;
		padding: 24px;
		overflow-y: auto;
		background: var(--canvas);
	}
	.transcript { display: grid; width: min(620px, 100%); justify-self: center; gap: 8px; }
	.transcript article { border: 1px solid var(--line); border-radius: var(--radius); padding: 10px; }
	.transcript article p, .transcript small { margin: 0; color: var(--text-muted); font-size: 12px; }
	.group-list { display: flex; flex-wrap: wrap; gap: 8px; }

	.live-copy,
	.chat-composer {
		display: grid;
		width: min(620px, 100%);
		justify-self: center;
		gap: 10px;
	}

	.chat-composer {
		position: relative;
	}

	:global(.chat-composer textarea) {
		min-height: 92px;
		resize: vertical;
	}

	:global(.chat-composer button) {
		position: absolute;
		right: 9px;
		bottom: 9px;
	}
</style>
