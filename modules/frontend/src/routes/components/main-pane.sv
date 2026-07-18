<script lang="ts" effect>
	import { Effect, Option } from "effect";
	import {
		IconArrowUp as ArrowUp,
		IconCircleCheck as CircleCheck,
		IconGitMerge as GitMerge,
		IconSparkles as Sparkles,
	} from "@tabler/icons-svelte";

	import {
		ActivateTab,
		CloseTab,
		ConfirmCloseTab,
		DeriveBreadcrumbs,
		DeriveTabOverflow,
		DoubleClickTab,
		OpenDiffPreview,
		OpenPreview,
		PinTab,
		SwitchMode,
		UpdateChatView,
		UpdateEditorView,
		UpdateOrchestratorView,
		type CloseTabOutcome,
		type DirtyCloseConfirmation,
		type TabMutationOutcome,
		type WorkspaceMode,
		type WorkspaceState,
		type WorkspaceTab,
	} from "$lib/workspace/workspace-tab-model";

	import FileTabStrip from "./file-tab-strip.sv";
	import ModeSwitcher from "./mode-switcher.sv";
	import QuickOpen from "./quick-open.sv";
	import WorkspaceNavigation from "./workspace-navigation.sv";
	import {
		CreateWorkspaceFixtureState,
		FileFixtureById,
		file_fixtures,
		type FileFixture,
	} from "./editor-fixtures";
	import { Button } from "$lib/components/ui/button";
	import { Textarea } from "$lib/components/ui/textarea";

	const orchestrator_nodes = [
		{ id: "node-sol", label: "Coordinate workspace UI", status: "Waiting" },
		{ id: "node-terra", label: "Implement fixture shell", status: "Working" },
		{ id: "node-luna", label: "Verify interaction seams", status: "Complete" },
	] as const;

	const ActiveTab = (state: WorkspaceState) =>
		Effect.gen(function* () {
			yield* Effect.void;

			const active_tab_id = Option.getOrUndefined(state.active_tab_id);
			for (const tab of state.tabs) {
				if (tab.id === active_tab_id) {
					return Option.some(tab);
				}
			}

			return Option.none<WorkspaceTab>();
		});

	const InitialWorkspaceView = Effect.gen(function* () {
		const workspace = yield* CreateWorkspaceFixtureState();
		const tab_overflow = yield* DeriveTabOverflow(workspace, 3);
		const active_tab = yield* ActiveTab(workspace);
		const active_fixture = yield* FileFixtureById(
			Option.isSome(active_tab) ? active_tab.value.file.id : file_fixtures[0]!.id,
		);
		const breadcrumbs = Option.isSome(active_tab)
			? yield* DeriveBreadcrumbs(active_tab.value.file)
			: [];

		return { workspace, tab_overflow, active_tab, active_fixture, breadcrumbs } as const;
	});

	const initial_view = yield* InitialWorkspaceView;
	let workspace = $state.raw(initial_view.workspace);
	let tab_overflow = $state.raw(initial_view.tab_overflow);
	let active_tab = $state.raw(initial_view.active_tab);
	let active_fixture = $state.raw<FileFixture>(initial_view.active_fixture);
	let breadcrumbs = $state.raw<ReadonlyArray<string>>(initial_view.breadcrumbs);
	let editor_scroll_top = $state(initial_view.workspace.editor.scroll_top);
	let chat_scroll_top = $state(initial_view.workspace.chat.transcript_scroll_top);
	let orchestrator_scroll_top = $state(initial_view.workspace.orchestrator.graph_scroll_top);
	let chat_draft = $state(initial_view.workspace.chat.draft);
	let selected_node_id = $state(
		Option.getOrUndefined(initial_view.workspace.orchestrator.selected_node_id) ?? "node-sol",
	);
	let editor_viewport = $state<HTMLDivElement>();
	let chat_viewport = $state<HTMLDivElement>();
	let orchestrator_viewport = $state<HTMLDivElement>();

	const RefreshWorkspace = (next_workspace: WorkspaceState) =>
		Effect.gen(function* () {
			workspace = next_workspace;
			tab_overflow = yield* DeriveTabOverflow(next_workspace, 3);
			active_tab = yield* ActiveTab(next_workspace);
			if (Option.isSome(active_tab)) {
				active_fixture = yield* FileFixtureById(active_tab.value.file.id);
				breadcrumbs = yield* DeriveBreadcrumbs(active_tab.value.file);
			} else {
				breadcrumbs = [];
			}

			yield* Effect.sleep(0);
			if (Option.isSome(active_tab)) {
				document
					.getElementById(`workspace-tab-${active_tab.value.generation}`)
					?.scrollIntoView({ block: "nearest", inline: "nearest" });
			}
		});

	const ApplyMutation = (outcome: TabMutationOutcome) =>
		Effect.gen(function* () {
			if (outcome._tag === "Updated") {
				yield* RefreshWorkspace(outcome.state);
			}
		});

	const SelectMode = (next_mode: WorkspaceMode) =>
		Effect.gen(function* () {
			yield* RefreshWorkspace(yield* SwitchMode(workspace, next_mode));
			yield* Effect.sleep(0);
			if (next_mode === "editor" && editor_viewport !== undefined) {
				editor_viewport.scrollTop = editor_scroll_top;
			} else if (next_mode === "chat" && chat_viewport !== undefined) {
				chat_viewport.scrollTop = chat_scroll_top;
			} else if (
				next_mode === "orchestrator" &&
				orchestrator_viewport !== undefined
			) {
				orchestrator_viewport.scrollTop = orchestrator_scroll_top;
			}
		});

	const ActivateWorkspaceTab = (tab_id: string) =>
		Effect.gen(function* () {
			yield* ApplyMutation(yield* ActivateTab(workspace, tab_id));
		});

	const PinWorkspaceTab = (tab_id: string) =>
		Effect.gen(function* () {
			yield* ApplyMutation(yield* PinTab(workspace, tab_id));
		});

	const PromoteWorkspaceTab = (tab_id: string) =>
		Effect.gen(function* () {
			yield* ApplyMutation(yield* DoubleClickTab(workspace, tab_id));
		});

	const RequestCloseWorkspaceTab = (tab_id: string): Effect.Effect<CloseTabOutcome> =>
		Effect.gen(function* () {
			const outcome = yield* CloseTab(workspace, tab_id);
			if (outcome._tag === "Closed") {
				yield* RefreshWorkspace(outcome.state);
			}

			return outcome;
		});

	const ConfirmCloseWorkspaceTab = (
		confirmation: DirtyCloseConfirmation,
	): Effect.Effect<CloseTabOutcome> =>
		Effect.gen(function* () {
			const outcome = yield* ConfirmCloseTab(workspace, confirmation);
			if (outcome._tag === "Closed") {
				yield* RefreshWorkspace(outcome.state);
			}

			return outcome;
		});

	const OpenRecentFixture = (file_id: string) =>
		Effect.gen(function* () {
			const file = yield* FileFixtureById(file_id);
			yield* RefreshWorkspace(yield* OpenPreview(workspace, file));
		});

	const OpenChangedFixture = (file_id: string) =>
		Effect.gen(function* () {
			for (const changed_file of workspace.changed_files) {
				if (changed_file.file.id === file_id) {
					const change_id = `fixture-review:${file_id}:${changed_file.change.added}:${changed_file.change.removed}`;
					yield* RefreshWorkspace(
						yield* OpenDiffPreview(workspace, changed_file.file, change_id),
					);
					return;
				}
			}
		});

	const CaptureEditorScroll = (scroll_top: number) =>
		Effect.gen(function* () {
			editor_scroll_top = scroll_top;
			workspace = yield* UpdateEditorView(workspace, {
				...workspace.editor,
				scroll_top,
			});
		});

	const CaptureChatScroll = (scroll_top: number) =>
		Effect.gen(function* () {
			chat_scroll_top = scroll_top;
			workspace = yield* UpdateChatView(workspace, {
				...workspace.chat,
				transcript_scroll_top: scroll_top,
			});
		});

	const UpdateChatDraft = (draft: string) =>
		Effect.gen(function* () {
			chat_draft = draft;
			workspace = yield* UpdateChatView(workspace, {
				...workspace.chat,
				draft,
			});
		});

	const CaptureOrchestratorScroll = (scroll_top: number) =>
		Effect.gen(function* () {
			orchestrator_scroll_top = scroll_top;
			workspace = yield* UpdateOrchestratorView(workspace, {
				...workspace.orchestrator,
				graph_scroll_top: scroll_top,
			});
		});

	const SelectNode = (node_id: string) =>
		Effect.gen(function* () {
			selected_node_id = node_id;
			workspace = yield* UpdateOrchestratorView(workspace, {
				...workspace.orchestrator,
				selected_node_id: Option.some(node_id),
			});
		});
</script>

<section class="main-pane" aria-label="Workspace">
	<header class="workspace-header">
		<div class="workspace-title">
			<strong>artisan-editor</strong>
			<span>codex/backend-services</span>
		</div>
		<div class="workspace-primary-controls">
			<QuickOpen files={file_fixtures} on_open={OpenRecentFixture} />
			<ModeSwitcher mode={workspace.mode} on_select={SelectMode} />
		</div>
	</header>

	{#if workspace.mode === "editor"}
		<div class="mode-surface editor-surface">
			<FileTabStrip
				visible_tabs={tab_overflow.visible}
				overflow_tabs={tab_overflow.overflow}
				active_tab_id={Option.getOrUndefined(workspace.active_tab_id)}
				on_activate={ActivateWorkspaceTab}
				on_pin={PinWorkspaceTab}
				on_promote={PromoteWorkspaceTab}
				on_close={RequestCloseWorkspaceTab}
				on_confirm_close={ConfirmCloseWorkspaceTab}
			/>
			<WorkspaceNavigation
				{breadcrumbs}
				recent_files={workspace.recent_files}
				changed_files={workspace.changed_files}
				overflow_tabs={tab_overflow.overflow}
				on_open_recent={OpenRecentFixture}
				on_open_changed={OpenChangedFixture}
				on_activate_overflow={ActivateWorkspaceTab}
			/>
			{#if Option.isSome(active_tab)}
				<div class="editor-meta">
					<span>{active_tab.value.file.path}</span>
					<span>{active_tab.value.file.language} / Fixture editor</span>
				</div>
				{#if active_tab.value.content._tag === "DiffPreview"}
					<div class="diff-preview-banner"><GitMerge size={13} stroke={1.8} aria-hidden="true" /><strong>Diff preview</strong><span>{active_tab.value.content.change_id}</span></div>
				{/if}
				<div class="code-scroll" bind:this={editor_viewport} onscroll={yield* CaptureEditorScroll(event.currentTarget.scrollTop)} role="textbox" aria-readonly="true" tabindex="0" aria-label={`Fixture code for ${active_fixture.name}`}>
					{#if active_tab.value.content._tag === "DiffPreview"}
						<div class="inline-diff" aria-label={`Before and after fixture diff for ${active_fixture.name}`}>
							<section class="diff-side removed" aria-label="Before fixture content">
								<header><strong>Before</strong><span>Removed</span></header>
								<pre><code>{#each active_fixture.before_lines ?? [] as line}<span class="diff-line"><span class="diff-marker">−</span><span class="line-number">{line.number}</span><span class="line-source">{line.code || " "}</span></span>{/each}</code></pre>
							</section>
							<section class="diff-side added" aria-label="After fixture content">
								<header><strong>After</strong><span>Added</span></header>
								<pre><code>{#each active_fixture.lines as line}<span class="diff-line"><span class="diff-marker">+</span><span class="line-number">{line.number}</span><span class="line-source">{line.code || " "}</span></span>{/each}</code></pre>
							</section>
						</div>
					{:else}
						<pre><code>{#each active_fixture.lines as line}<span class="code-line"><span class="line-number">{line.number}</span><span class="line-source">{line.code || " "}</span></span>{/each}</code></pre>
					{/if}
				</div>
				<footer class="editor-status"><span>Ln {workspace.editor.cursor_line}, Col {workspace.editor.cursor_column}</span><span>Spaces: 2</span><span>UTF-8</span><span>Fixture workspace</span></footer>
			{:else}
				<div class="empty-editor"><p>No file is open.</p><span>Use Quick open to inspect a fixture file.</span></div>
			{/if}
		</div>
	{:else if workspace.mode === "chat"}
		<div class="mode-surface quiet-surface chat-surface" bind:this={chat_viewport} onscroll={yield* CaptureChatScroll(event.currentTarget.scrollTop)}>
			<div class="quiet-copy">
				<span class="surface-icon"><Sparkles size={19} stroke={1.7} aria-hidden="true" /></span>
				<p class="eyebrow">Fixture conversation</p>
				<h1>Work through the code without leaving the editor.</h1>
				<p>This fixture transcript proves the mode-owned viewport while typed transport work remains outside this surface.</p>
				<div class="fixture-transcript"><strong>Sol</strong><p>The active file and editor position remain intact while this conversation is open.</p><strong>Terra</strong><p>The draft and transcript restore when you switch back.</p></div>
			</div>
			<label class="chat-composer">
				<span>Message fixture</span>
				<Textarea value={chat_draft} oninput={yield* UpdateChatDraft(event.currentTarget.value)} />
				<Button size="icon-xs" type="button" aria-label="Send fixture message" disabled><ArrowUp size={16} stroke={1.8} aria-hidden="true" /></Button>
			</label>
		</div>
	{:else}
		<div class="mode-surface quiet-surface orchestrator-surface" bind:this={orchestrator_viewport} onscroll={yield* CaptureOrchestratorScroll(event.currentTarget.scrollTop)}>
			<div class="quiet-copy">
				<span class="surface-icon"><GitMerge size={19} stroke={1.7} aria-hidden="true" /></span>
				<p class="eyebrow">Fixture orchestration</p>
				<h1>One visible checkout, coordinated work.</h1>
				<p>The selected fixture node and graph viewport are mode-owned and restore when you return.</p>
			</div>
			<div class="stage-list" aria-label="Fixture orchestration nodes">
				{#each orchestrator_nodes as node, index}
				<Button variant="outline" class={selected_node_id === node.id ? "border-primary bg-primary/10 text-foreground" : ""} onclick={yield* SelectNode(node.id)}>
						<span class="stage-index">0{index + 1}</span><span>{node.label}</span><span>{node.status}</span>{#if node.status === "Complete"}<CircleCheck size={15} stroke={1.7} aria-label="Complete" />{/if}
				</Button>
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
		padding: 0 var(--pane-action-space, 10px) 0 14px;
		border-bottom: 1px solid var(--line);
	}

	.workspace-title,
	.workspace-primary-controls {
		display: flex;
		min-width: 0;
		align-items: center;
		gap: 8px;
	}

	.workspace-title {
		align-items: baseline;
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
		min-height: 28px;
		border-bottom: 1px solid var(--line);
	}

	.editor-status {
		min-height: 25px;
		justify-content: flex-end;
		border-top: 1px solid var(--line);
		background: var(--pane-inset);
	}

	.diff-preview-banner {
		display: flex;
		min-height: 27px;
		align-items: center;
		gap: 7px;
		padding: 0 12px;
		border-bottom: 1px solid color-mix(in oklch, var(--run-active) 48%, var(--line));
		background: color-mix(in oklch, var(--run-active) 10%, var(--pane));
		color: var(--run-active);
		font-size: 9px;
	}

	.diff-preview-banner span {
		overflow: hidden;
		color: var(--text-muted);
		font-family: var(--font-mono);
		text-overflow: ellipsis;
		white-space: nowrap;
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
		min-height: calc(100% + 180px);
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

	.inline-diff {
		display: grid;
		min-width: 760px;
		grid-template-columns: repeat(2, minmax(0, 1fr));
	}

	.diff-side {
		min-width: 0;
	}

	.diff-side + .diff-side {
		border-left: 1px solid var(--line);
	}

	.diff-side header {
		display: flex;
		height: 30px;
		align-items: center;
		justify-content: space-between;
		padding: 0 12px;
		border-bottom: 1px solid var(--line);
		font-size: 9px;
		text-transform: uppercase;
		letter-spacing: 0.06em;
	}

	.diff-side.removed header,
	.diff-side.removed .diff-marker {
		color: var(--diff-removed);
	}

	.diff-side.added header,
	.diff-side.added .diff-marker {
		color: var(--diff-added);
	}

	.diff-line {
		display: grid;
		grid-template-columns: 22px 42px minmax(0, 1fr);
		min-height: 21px;
	}

	.diff-side.removed .diff-line {
		background: var(--diff-removed-surface);
	}

	.diff-side.added .diff-line {
		background: var(--diff-added-surface);
	}

	.diff-marker {
		padding-left: 8px;
		font-weight: 700;
	}

	.empty-editor {
		display: grid;
		flex: 1;
		place-content: center;
		text-align: center;
	}

	.empty-editor p {
		margin: 0 0 5px;
	}

	.empty-editor span {
		color: var(--text-muted);
		font-size: 11px;
	}

	.quiet-surface {
		align-items: center;
		gap: 30px;
		padding: 44px;
		overflow-y: auto;
		background: var(--canvas);
	}

	.chat-surface::after,
	.orchestrator-surface::after {
		content: "";
		width: 1px;
		min-height: 320px;
		flex: 0 0 320px;
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

	.quiet-copy > p {
		max-width: 460px;
		margin: 14px 0 0;
		color: var(--text-muted);
		font-size: 12px;
		line-height: 1.6;
	}

	.fixture-transcript {
		display: grid;
		gap: 5px;
		margin-top: 28px;
		padding: 12px;
		border: 1px solid var(--line);
		border-radius: var(--radius-md);
		background: var(--pane);
		font-size: 10px;
	}

	.fixture-transcript p {
		margin: 0 0 8px;
		color: var(--text-muted);
		line-height: 1.45;
	}

	.chat-composer {
		position: relative;
		display: grid;
		width: min(620px, 100%);
		gap: 7px;
		color: var(--text-muted);
		font-size: 10px;
	}

	:global(.chat-composer textarea) {
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

	:global(.chat-composer button) {
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

	:global(.chat-composer textarea:focus-visible),
	:global(.stage-list button:focus-visible) {
		outline: 2px solid var(--focus);
		outline-offset: 2px;
	}

	.stage-list {
		display: grid;
		width: min(620px, 100%);
		gap: 6px;
	}

	:global(.stage-list button) {
		display: grid;
		grid-template-columns: 28px minmax(0, 1fr) auto auto;
		align-items: center;
		gap: 9px;
		min-height: 42px;
		padding: 0 12px;
		border: 1px solid var(--line);
		border-radius: var(--radius-sm);
		background: var(--pane);
		color: var(--text-secondary);
		font-size: 11px;
		text-align: left;
		cursor: pointer;
	}

	.stage-index {
		color: var(--text-muted);
		font-family: var(--font-mono);
		font-size: 9px;
	}

	@media (max-width: 799px) {
		.workspace-title span {
			display: none;
		}

		.quiet-surface {
			padding: 28px 18px;
		}
	}
</style>
