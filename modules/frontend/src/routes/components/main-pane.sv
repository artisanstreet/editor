<script lang="ts" effect>
	import { Effect, Option } from "effect";
	import {
		IconArrowUp as ArrowUp,
		IconFileCode as FileCode,
		IconGitMerge as GitMerge,
		IconMessage as Message,
	} from "@tabler/icons-svelte";

	import type { LiveWorkspaceSnapshot } from "$lib/live-workspace/store";
	import type { WorkspaceMode } from "$lib/workspace/workspace-tab-model";
	import { Button } from "$lib/components/ui/button";
	import { Textarea } from "$lib/components/ui/textarea";

	import ModeSwitcher from "./mode-switcher.sv";

	let {
		live_snapshot,
		on_send_live_message,
	}: {
		live_snapshot: LiveWorkspaceSnapshot;
		on_send_live_message: (text: string) => Effect.Effect<void>;
	} = $props();

	let mode = $state<WorkspaceMode>("editor");
	let chat_draft = $state("");

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
</script>

<section class="main-pane" aria-label="Workspace">
	<header class="workspace-header">
		<div class="workspace-title">
			<strong>Workspace</strong>
			<span>{live_snapshot.phase}</span>
		</div>
		<ModeSwitcher {mode} on_select={SelectMode} />
	</header>

	{#if mode === "editor"}
		<div class="mode-surface empty-surface">
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
					<p>Select an existing thread to message its active Codex run. Transcript history is unavailable until its authoritative projection is connected.</p>
				{/if}
			</div>
			<label class="chat-composer">
				<span>Message active Codex run</span>
				<Textarea bind:value={chat_draft} aria-label="Message active Codex run" onkeydown={yield* HandleComposerKey(event)} />
				<Button size="icon-xs" type="button" aria-label="Send message" disabled={chat_draft.trim().length === 0 || Option.isNone(live_snapshot.thread_work)} onclick={yield* SendLiveMessage()}><ArrowUp size={16} aria-hidden="true" /></Button>
			</label>
		</div>
	{:else}
		<div class="mode-surface empty-surface">
			<GitMerge size={22} aria-hidden="true" />
			<h1>Orchestrator ready</h1>
			<p>The live agent graph will appear here when the authoritative orchestration projection is connected.</p>
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
