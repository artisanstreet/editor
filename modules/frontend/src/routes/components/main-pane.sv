<script lang="ts" effect>
	import { Effect } from "effect";
	import { IconArrowUp as ArrowUp, IconCircleCheck as CircleCheck, IconGitMerge as GitMerge, IconSparkles as Sparkles } from "@tabler/icons-svelte";

	import FileTabStrip from "./file-tab-strip.sv";
	import ModeSwitcher from "./mode-switcher.sv";
	import { file_fixtures, type EditorMode } from "./editor-fixtures";

	let mode = $state<EditorMode>("editor");
	let active_file_index = $state(0);
	let chat_draft = $state("Can you explain the workspace service boundary?");
	let selected_stage = $state("Implement fixture shell");

	const active_file = $derived(file_fixtures[active_file_index]);
	const active_file_id = $derived(active_file.id);

	const SelectMode = (next_mode: EditorMode) =>
		Effect.gen(function* () {
			mode = next_mode;
		});

	const SelectFile = (file_index: number) =>
		Effect.gen(function* () {
			active_file_index = file_index;
		});

	const UpdateChatDraft = (value: string) =>
		Effect.gen(function* () {
			chat_draft = value;
		});

	const SelectStage = (stage: string) =>
		Effect.gen(function* () {
			selected_stage = stage;
		});
</script>

<section class="main-pane" aria-label="Workspace">
	<header class="workspace-header">
		<div class="workspace-title">
			<strong>artisan-editor</strong>
			<span>codex/backend-services</span>
		</div>
		<ModeSwitcher {mode} on_select={SelectMode} />
	</header>

	{#if mode === "editor"}
		<div class="mode-surface editor-surface">
			<FileTabStrip files={file_fixtures} {active_file_id} on_select={SelectFile} />
			<div class="editor-meta">
				<span>{active_file.path}</span>
				<span>{active_file.language} / Fixture editor</span>
			</div>
			<div class="code-scroll" role="textbox" aria-readonly="true" tabindex="0" aria-label={`Fixture code for ${active_file.name}`}>
				<pre><code>{#each active_file.lines as line}<span class="code-line"><span class="line-number">{line.number}</span><span class="line-source">{line.code || " "}</span></span>{/each}</code></pre>
			</div>
			<footer class="editor-status"><span>Ln 7, Col 31</span><span>Spaces: 2</span><span>UTF-8</span><span>Preview fixture</span></footer>
		</div>
	{:else if mode === "chat"}
		<div class="mode-surface quiet-surface chat-surface">
			<div class="quiet-copy">
				<span class="surface-icon"><Sparkles size={19} stroke={1.7} aria-hidden="true" /></span>
				<p class="eyebrow">Fixture conversation</p>
				<h1>Work through the code without leaving the editor.</h1>
				<p>This quiet state reserves the conversation canvas while backend transport is still being completed.</p>
			</div>
			<label class="chat-composer">
				<span>Message fixture</span>
				<textarea value={chat_draft} oninput={yield* UpdateChatDraft(event.currentTarget.value)}></textarea>
				<button type="button" aria-label="Send fixture message" disabled><ArrowUp size={16} stroke={1.8} aria-hidden="true" /></button>
			</label>
		</div>
	{:else}
		<div class="mode-surface quiet-surface orchestrator-surface">
			<div class="quiet-copy">
				<span class="surface-icon"><GitMerge size={19} stroke={1.7} aria-hidden="true" /></span>
				<p class="eyebrow">Fixture orchestration</p>
				<h1>One visible checkout, coordinated work.</h1>
				<p>The graph is intentionally static until the orchestration projection is wired into the frontend boundary.</p>
			</div>
			<div class="stage-list" aria-label="Fixture orchestration stages">
				{#each ["Inspect backend contracts", "Implement fixture shell", "Verify responsive behavior"] as stage, index}
					<button class:active={selected_stage === stage} type="button" onclick={yield* SelectStage(stage)}>
						<span class="stage-index">0{index + 1}</span><span>{stage}</span>{#if index === 0}<CircleCheck size={15} stroke={1.7} aria-label="Complete" />{/if}
					</button>
				{/each}
			</div>
		</div>
	{/if}
</section>

<style>
	.main-pane {
		display: flex;
		flex-direction: column;
		height: 100%;
		min-height: 0;
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
		padding: 0 10px 0 14px;
		border-bottom: 1px solid var(--line);
	}

	.workspace-title {
		display: flex;
		min-width: 0;
		align-items: baseline;
		gap: 9px;
	}

	.workspace-title strong {
		font-size: 12px;
	}

	.workspace-title span {
		overflow: hidden;
		color: var(--text-muted);
		font-size: 10px;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.mode-surface {
		display: flex;
		flex: 1;
		min-height: 0;
		flex-direction: column;
	}

	.editor-meta,
	.editor-status {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 14px;
		padding: 0 12px;
		color: var(--text-muted);
		font-size: 10px;
	}

	.editor-meta {
		min-height: 30px;
		border-bottom: 1px solid var(--line);
	}

	.editor-status {
		min-height: 25px;
		justify-content: flex-end;
		border-top: 1px solid var(--line);
		background: var(--pane-inset);
	}

	.code-scroll {
		flex: 1;
		min-height: 0;
		overflow: auto;
		overscroll-behavior: contain;
		background: var(--canvas);
	}

	.code-scroll:focus-visible {
		outline: 2px solid var(--focus);
		outline-offset: -2px;
	}

	pre {
		min-width: max-content;
		margin: 0;
		padding: 18px 0 48px;
		font: 12px/1.75 var(--font-mono);
		tab-size: 2;
	}

	.code-line {
		display: grid;
		grid-template-columns: 52px minmax(0, 1fr);
		min-height: 21px;
	}

	.code-line:nth-child(7) {
		background: var(--selection);
	}

	.line-number {
		padding-right: 14px;
		color: var(--text-muted);
		text-align: right;
		user-select: none;
	}

	.line-source {
		padding-right: 40px;
		color: var(--text-secondary);
		white-space: pre;
	}

	.quiet-surface {
		align-items: center;
		justify-content: center;
		gap: 30px;
		padding: 44px;
		overflow-y: auto;
		background: var(--canvas);
	}

	.quiet-copy {
		width: min(520px, 100%);
	}

	.surface-icon {
		display: grid;
		width: 36px;
		height: 36px;
		place-items: center;
		border: 1px solid var(--line-strong);
		border-radius: var(--radius-md);
		background: var(--raised);
	}

	.eyebrow {
		margin: 16px 0 6px;
		color: var(--text-muted);
		font-size: 10px;
		font-weight: 650;
		text-transform: uppercase;
		letter-spacing: 0.08em;
	}

	h1 {
		max-width: 480px;
		margin: 0;
		font-size: clamp(24px, 4vw, 42px);
		font-weight: 630;
		line-height: 1;
		letter-spacing: -0.045em;
	}

	.quiet-copy > p:last-child {
		max-width: 460px;
		margin: 14px 0 0;
		color: var(--text-muted);
		font-size: 12px;
		line-height: 1.6;
	}

	.chat-composer {
		position: relative;
		display: grid;
		width: min(620px, 100%);
		gap: 7px;
		color: var(--text-muted);
		font-size: 10px;
	}

	.chat-composer textarea {
		min-height: 92px;
		resize: vertical;
		padding: 13px 44px 13px 13px;
		border: 1px solid var(--line-strong);
		border-radius: var(--radius-md);
		background: var(--raised);
		color: var(--text-primary);
		font: inherit;
		font-size: 12px;
	}

	.chat-composer button {
		position: absolute;
		right: 9px;
		bottom: 9px;
		display: grid;
		width: 28px;
		height: 28px;
		place-items: center;
		border: 0;
		border-radius: var(--radius-sm);
		background: var(--text-primary);
		color: var(--canvas);
	}

	.chat-composer textarea:focus-visible,
	.stage-list button:focus-visible {
		outline: 2px solid var(--focus);
		outline-offset: 2px;
	}

	.stage-list {
		display: grid;
		width: min(620px, 100%);
		gap: 6px;
	}

	.stage-list button {
		display: grid;
		grid-template-columns: 28px minmax(0, 1fr) auto;
		align-items: center;
		gap: 9px;
		min-height: 42px;
		padding: 0 12px;
		border: 1px solid var(--line);
		border-radius: var(--radius-sm);
		background: var(--pane);
		color: var(--text-secondary);
		font: inherit;
		font-size: 11px;
		text-align: left;
		cursor: pointer;
	}

	.stage-list button.active {
		border-color: var(--line-strong);
		background: var(--selection);
		color: var(--text-primary);
	}

	.stage-index {
		color: var(--text-muted);
		font-family: var(--font-mono);
		font-size: 9px;
	}

	@media (max-width: 1279px) {
		.workspace-header {
			padding-right: 48px;
		}
	}

	@media (max-width: 799px) {
		.workspace-header {
			padding-right: 82px;
		}

		.workspace-title span {
			display: none;
		}

		.quiet-surface {
			padding: 28px 18px;
		}
	}
</style>
