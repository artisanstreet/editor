<script lang="ts" effect>
	import { Effect, Option } from "effect";
	import { runFork } from "effect/Effect";
	import { IconDeviceFloppy as Save, IconRefresh as Refresh } from "@tabler/icons-svelte";
	import type { WorkspaceFileReadQueryResult } from "@artisan/protocol";
	import type {
		ArtisanWorkspaceFileDiscoveryInput,
		ArtisanWorkspaceFileReadInput,
		ArtisanWorkspaceFileReplaceInput,
	} from "@artisan/transport/client";
	import type {
		LiveWorkspaceActions,
		LiveWorkspaceFileReplaceOutcome,
		LiveWorkspaceSnapshot,
	} from "$lib/live-workspace/store";
	import {
		MonacoEditorService,
		MonacoFileKeyForFile,
		MonacoLanguageForPath,
		type MonacoWorkspaceFile,
	} from "$lib/editor/monaco-editor-service";
	import { MakeMonacoEditorMount } from "$lib/editor/monaco-editor-mount";
	import MonacoEditor from "$lib/components/editor/monaco-editor.svelte";
	import { Alert, AlertDescription, AlertTitle } from "$lib/components/ui/alert";
	import { Button } from "$lib/components/ui/button";
	import type {
		DirtyCloseConfirmation,
		WorkspaceFileReference,
		WorkspaceMode,
		WorkspaceState,
	} from "$lib/workspace/workspace-tab-model";
	import {
		ActivateTab,
		CloseTab,
		ConfirmCloseTab,
		CreateWorkspaceState,
		OpenFile as OpenWorkspaceFile,
		PinTab,
		PromoteTab,
	} from "$lib/workspace/workspace-tab-model";
	import ChatTranscript from "./chat-transcript.sv";
	import FileTabStrip from "./file-tab-strip.sv";
	import ModeSwitcher from "./mode-switcher.sv";
	import OrchestratorGraph from "./orchestrator-graph.sv";
	import QuickOpen from "./quick-open.sv";

	let {
		live_snapshot,
		on_send_live_message,
		on_refresh_workspace_files,
		on_read_workspace_file,
		on_replace_workspace_file,
		on_select_orchestration_group,
		actions,
	}: {
		live_snapshot: LiveWorkspaceSnapshot;
		on_send_live_message: (text: string) => Effect.Effect<void>;
		on_refresh_workspace_files?: (
			input: ArtisanWorkspaceFileDiscoveryInput,
		) => Effect.Effect<void>;
		on_read_workspace_file?: (
			input: ArtisanWorkspaceFileReadInput,
		) => Effect.Effect<Option.Option<WorkspaceFileReadQueryResult>>;
		on_replace_workspace_file?: (
			input: ArtisanWorkspaceFileReplaceInput,
		) => Effect.Effect<LiveWorkspaceFileReplaceOutcome>;
		on_select_orchestration_group?: (group_id: string) => Effect.Effect<void>;
		actions: Pick<LiveWorkspaceActions, "Command" | "OpenAsset" | "ResolveRichLink">;
	} = $props();

	const monaco = yield* MonacoEditorService;
	const monaco_mount = MakeMonacoEditorMount(monaco);
	const initial_workspace_state = yield* CreateWorkspaceState();
	type WorkspaceContext = {
		readonly chat_draft: string;
		readonly mode: WorkspaceMode;
		readonly state: WorkspaceState;
	};
	const workspace_contexts = new Map<string, WorkspaceContext>();
	let mode = $state<WorkspaceMode>("editor");
	let chat_draft = $state("");
	let workspace_state = $state.raw(initial_workspace_state);
	let active_workspace_context_key = $state<string>();
	let save_error = $state<string>();
	const loaded_files = new Map<string, WorkspaceFileReadQueryResult>();

	const selected_thread_id = $derived(
		Option.getOrUndefined(live_snapshot.selected_thread_id),
	);
	const project = $derived(
		live_snapshot.threads.find(
			(thread) => thread.thread_id === selected_thread_id,
		)?.primary_project,
	);
	const workspace_context_key = $derived(
		selected_thread_id === undefined
			? undefined
			: JSON.stringify([selected_thread_id, project?.project_id ?? "unassigned"]),
	);
	const run = $derived(Option.getOrUndefined(live_snapshot.thread_work));
	const files = $derived(
		(Option.getOrUndefined(live_snapshot.workspace_file_page)?.entries ?? [])
			.filter((entry) => entry.kind === "file")
			.map((entry) => ({
				id: entry.path,
				name: entry.path.split("/").at(-1) ?? entry.path,
				language: MonacoLanguageForPath(entry.path, "plaintext"),
				path: entry.path,
			})),
	);
	const active_tab_id = $derived(Option.getOrUndefined(workspace_state.active_tab_id));
	const active_tab = $derived(
		workspace_state.tabs.find((tab) => tab.id === active_tab_id),
	);
	const visible_tabs = $derived(workspace_state.tabs.slice(0, 6));
	const overflow_tabs = $derived(workspace_state.tabs.slice(6));

	const ToMonacoFile = (
		file: WorkspaceFileReference,
		loaded: WorkspaceFileReadQueryResult,
	): MonacoWorkspaceFile => ({
		content: loaded.content,
		id: file.id,
		language: file.language,
		path: file.path,
		revision: loaded.identity.content_hash,
		workspace_id: loaded.workspace_id,
	});
	const LoadedFileKey = (workspace_id: string, file_id: string) =>
		MonacoFileKeyForFile({ id: file_id, workspace_id });

	const RestoreWorkspaceFile = (
		expected_context_key: string,
		workspace_id: string,
		file: WorkspaceFileReference,
	) =>
		Effect.gen(function* () {
			if (on_read_workspace_file === undefined) return;
			const loaded = yield* on_read_workspace_file({ workspace_id, path: file.path });
			if (
				Option.isNone(loaded) ||
				active_workspace_context_key !== expected_context_key
			)
				return;
			loaded_files.set(LoadedFileKey(workspace_id, file.id), loaded.value);
			const activation = yield* monaco.Activate(ToMonacoFile(file, loaded.value));
			save_error =
				activation._tag === "Conflict"
					? `${file.path} changed in the workspace while local edits were open.`
					: undefined;
		});

	$effect(() => {
		const next_context_key = workspace_context_key;
		if (next_context_key === active_workspace_context_key) return;

		if (active_workspace_context_key !== undefined) {
			workspace_contexts.set(active_workspace_context_key, {
				chat_draft,
				mode,
				state: workspace_state,
			});
		}

		const restored =
			next_context_key === undefined
				? undefined
				: workspace_contexts.get(next_context_key);
		mode = restored?.mode ?? "editor";
		chat_draft = restored?.chat_draft ?? "";
		workspace_state = restored?.state ?? { ...initial_workspace_state };
		save_error = undefined;
		active_workspace_context_key = next_context_key;

		const restored_tab_id = Option.getOrUndefined(workspace_state.active_tab_id);
		const restored_tab = workspace_state.tabs.find((tab) => tab.id === restored_tab_id);
		if (
			next_context_key !== undefined &&
			project !== undefined &&
			restored_tab !== undefined
		) {
			runFork(
				RestoreWorkspaceFile(
					next_context_key,
					project.project_id,
					restored_tab.file,
				),
			);
		}
	});

	const SelectMode = (next: WorkspaceMode) => Effect.sync(() => (mode = next));
	const RefreshFiles = Effect.gen(function* () {
		if (project === undefined || on_refresh_workspace_files === undefined) return;
		yield* on_refresh_workspace_files({ workspace_id: project.project_id, limit: 1000 });
	});

	const ReadAndActivate = (file: WorkspaceFileReference) =>
		Effect.gen(function* () {
			if (project === undefined || on_read_workspace_file === undefined) return false;
			const loaded = yield* on_read_workspace_file({
				workspace_id: project.project_id,
				path: file.path,
			});
			if (Option.isNone(loaded)) return false;
			loaded_files.set(LoadedFileKey(loaded.value.workspace_id, file.id), loaded.value);
			const activation = yield* monaco.Activate(ToMonacoFile(file, loaded.value));
			save_error =
				activation._tag === "Conflict"
					? `${file.path} changed in the workspace while local edits were open.`
					: undefined;
			return true;
		});

	const OpenFile = (file_id: string) =>
		Effect.gen(function* () {
			const file = files.find((item) => item.id === file_id);
			if (file === undefined || !(yield* ReadAndActivate(file))) return;
			workspace_state = yield* OpenWorkspaceFile(workspace_state, file);
		});

	const ActivateFileTab = (tab_id: string) =>
		Effect.gen(function* () {
			const outcome = yield* ActivateTab(workspace_state, tab_id);
			if (outcome._tag !== "Updated") return;
			const tab = outcome.state.tabs.find((candidate) => candidate.id === tab_id);
			if (tab === undefined || !(yield* ReadAndActivate(tab.file))) return;
			workspace_state = outcome.state;
		});

	const PinFileTab = (tab_id: string) =>
		Effect.gen(function* () {
			const outcome = yield* PinTab(workspace_state, tab_id);
			workspace_state = outcome.state;
		});

	const PromoteFileTab = (tab_id: string) =>
		Effect.gen(function* () {
			const outcome = yield* PromoteTab(workspace_state, tab_id);
			workspace_state = outcome.state;
		});

	const CloseFileTab = (tab_id: string) =>
		Effect.gen(function* () {
			const tab = workspace_state.tabs.find((candidate) => candidate.id === tab_id);
			const loaded =
				tab === undefined || project === undefined
					? undefined
					: loaded_files.get(LoadedFileKey(project.project_id, tab.file.id));
			const outcome = yield* CloseTab(workspace_state, tab_id);
			if (outcome._tag === "Closed" && tab !== undefined) {
				const workspace_id = loaded?.workspace_id ?? project?.project_id;
				if (workspace_id !== undefined) {
					yield* monaco.Close({ id: tab.file.id, workspace_id });
					loaded_files.delete(LoadedFileKey(workspace_id, tab.file.id));
				}
			}
			workspace_state = outcome.state;
			return outcome;
		});

	const ConfirmCloseFileTab = (confirmation: DirtyCloseConfirmation) =>
		Effect.gen(function* () {
			const tab = workspace_state.tabs.find(
				(candidate) => candidate.id === confirmation.tab_id,
			);
			const loaded =
				tab === undefined || project === undefined
					? undefined
					: loaded_files.get(LoadedFileKey(project.project_id, tab.file.id));
			const outcome = yield* ConfirmCloseTab(workspace_state, confirmation);
			if (outcome._tag === "Closed" && tab !== undefined) {
				const workspace_id = loaded?.workspace_id ?? project?.project_id;
				if (workspace_id !== undefined) {
					yield* monaco.Close({ id: tab.file.id, workspace_id });
					loaded_files.delete(LoadedFileKey(workspace_id, tab.file.id));
				}
			}
			workspace_state = outcome.state;
			return outcome;
		});

	const SyncDirtyTabs = Effect.gen(function* () {
		if (project === undefined) return;
		const state = yield* monaco.Current;
		workspace_state = {
			...workspace_state,
			tabs: workspace_state.tabs.map((tab) => ({
				...tab,
				edit_state: state.dirty_file_keys.has(
					MonacoFileKeyForFile({ id: tab.file.id, workspace_id: project.project_id }),
				)
					? {
							_tag: "Dirty" as const,
							revision:
								tab.edit_state._tag === "Dirty" ? tab.edit_state.revision : 1,
						}
					: { _tag: "Clean" as const },
			})),
		};
	});

	const SaveActiveFile = Effect.gen(function* () {
		if (
			active_tab === undefined ||
			project === undefined ||
			run === undefined ||
			selected_thread_id === undefined ||
			on_replace_workspace_file === undefined
		)
			return;
		const loaded = loaded_files.get(
			LoadedFileKey(project.project_id, active_tab.file.id),
		);
		if (loaded === undefined) return;

		const outcome = yield* monaco.Save(
			{ id: active_tab.file.id, workspace_id: loaded.workspace_id },
			loaded.identity.content_hash,
			(file) =>
				on_replace_workspace_file({
					agent_id: run.agent_id,
					change_id: `change_${globalThis.crypto.randomUUID()}`,
					content: file.content,
					expected_before: loaded.identity,
					path: file.path,
					run_id: run.run_id,
					thread_id: selected_thread_id,
					workspace_id: loaded.workspace_id,
				}).pipe(
					Effect.map((result) => {
						if (result._tag === "Saved") {
							loaded_files.set(
								LoadedFileKey(result.file.workspace_id, active_tab.file.id),
								result.file,
							);
							return { _tag: "Saved" as const, file: ToMonacoFile(active_tab.file, result.file) };
						}
						if (result._tag === "Conflict") {
							loaded_files.set(
								LoadedFileKey(result.file.workspace_id, active_tab.file.id),
								result.file,
							);
							save_error = result.message;
							return {
								_tag: "Conflict" as const,
								current_revision: result.file.identity.content_hash,
								file: ToMonacoFile(active_tab.file, result.file),
							};
						}
						save_error = result.message;
						return {
							_tag: "Conflict" as const,
							current_revision: loaded.identity.content_hash,
							file,
						};
					}),
				),
		);
		if (outcome._tag === "Saved") save_error = undefined;
		yield* SyncDirtyTabs;
	});

	const HandleEditorKey = (event: KeyboardEvent) =>
		Effect.gen(function* () {
			if (mode !== "editor") return;
			if (event.key.toLowerCase() === "s" && (event.ctrlKey || event.metaKey)) {
				event.preventDefault();
				yield* SaveActiveFile;
				return;
			}
			yield* SyncDirtyTabs;
		});

	const SelectGroup = (group_id: string) =>
		on_select_orchestration_group?.(group_id) ?? Effect.void;
</script>

<svelte:window onkeyup={yield* HandleEditorKey(event)} oninput={yield* SyncDirtyTabs} onpaste={yield* SyncDirtyTabs} oncut={yield* SyncDirtyTabs} ondrop={yield* SyncDirtyTabs} />

<section class="flex h-full min-h-0 flex-col overflow-hidden rounded-lg border bg-card" aria-label="Workspace">
	<header class="flex min-h-12 items-center justify-between gap-3 border-b px-3">
		<div class="min-w-0"><strong class="text-sm">Workspace</strong><span class="ml-2 text-xs text-muted-foreground">{live_snapshot.phase}</span></div>
		<div class="flex items-center gap-2">
			{#if mode === "editor"}<QuickOpen {files} on_open={OpenFile} /><Button variant="outline" size="icon-sm" aria-label="Refresh workspace files" disabled={project === undefined || on_refresh_workspace_files === undefined} onclick={yield* RefreshFiles}><Refresh size={15} /></Button>{/if}
			<ModeSwitcher {mode} on_select={SelectMode} />
		</div>
	</header>
	{#if mode === "editor"}
		<div class="flex min-h-0 flex-1 flex-col">
			{#if workspace_state.tabs.length > 0}
				<FileTabStrip {visible_tabs} {overflow_tabs} {active_tab_id} on_activate={ActivateFileTab} on_pin={PinFileTab} on_promote={PromoteFileTab} on_close={CloseFileTab} on_confirm_close={ConfirmCloseFileTab} />
			{/if}
			{#if active_tab !== undefined}
				<div class="flex h-10 items-center gap-2 border-b px-3"><span class="truncate text-xs">{active_tab.file.path}</span><Button class="ml-auto" variant="ghost" size="icon-sm" aria-label="Save active file" title="Save (Ctrl+S)" disabled={on_replace_workspace_file === undefined || run === undefined} onclick={yield* SaveActiveFile}><Save size={15} /></Button></div>
				{#if save_error}<Alert variant="destructive" class="m-2"><AlertTitle>File conflict</AlertTitle><AlertDescription>{save_error}</AlertDescription></Alert>{/if}
				<MonacoEditor mount={monaco_mount} label={`Editing ${active_tab.file.path}`} />
			{:else}
				<div class="grid flex-1 place-content-center gap-2 p-6 text-center"><p class="text-sm font-medium">Open a workspace file</p><p class="text-xs text-muted-foreground">Use Quick open or refresh the selected project's bounded file list.</p></div>
			{/if}
		</div>
	{:else if mode === "chat"}
		<ChatTranscript snapshot={live_snapshot} bind:draft={chat_draft} on_send={on_send_live_message} {actions} />
	{:else}
		<OrchestratorGraph snapshot={live_snapshot} on_select_group={SelectGroup} {actions} />
	{/if}
</section>
