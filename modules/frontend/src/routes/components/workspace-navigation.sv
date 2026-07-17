<script lang="ts" effect>
	import { Effect, Option } from "effect";
	import { IconChevronRight as ChevronRight, IconFolderCode as FolderCode } from "@tabler/icons-svelte";

	import type {
		ChangedFile,
		WorkspaceFileReference,
		WorkspaceTab,
	} from "$lib/workspace/workspace-tab-model";

	let {
		breadcrumbs,
		recent_files,
		changed_files,
		overflow_tabs,
		on_open_recent,
		on_open_changed,
		on_activate_overflow,
	}: {
		breadcrumbs: ReadonlyArray<string>;
		recent_files: ReadonlyArray<WorkspaceFileReference>;
		changed_files: ReadonlyArray<ChangedFile>;
		overflow_tabs: ReadonlyArray<WorkspaceTab>;
		on_open_recent: (file_id: string) => Effect.Effect<void>;
		on_open_changed: (file_id: string) => Effect.Effect<void>;
		on_activate_overflow: (tab_id: string) => Effect.Effect<void>;
	} = $props();
	let recent_selection = $state("");
	let changed_selection = $state("");
	let overflow_selection = $state("");

	const SelectRecent = (file_id: string) =>
		Effect.gen(function* () {
			if (file_id.length > 0) {
				yield* on_open_recent(file_id);
			}
			recent_selection = "";
		});

	const SelectChanged = (file_id: string) =>
		Effect.gen(function* () {
			if (file_id.length > 0) {
				yield* on_open_changed(file_id);
			}
			changed_selection = "";
		});

	const SelectOverflow = (tab_id: string) =>
		Effect.gen(function* () {
			if (tab_id.length > 0) {
				yield* on_activate_overflow(tab_id);
			}
			overflow_selection = "";
		});

	const TabOptionLabel = (tab: WorkspaceTab) =>
		Effect.gen(function* () {
			yield* Effect.void;

			const states: Array<string> = [];
			if (tab.content._tag === "DiffPreview") {
				states.push("Diff preview");
			}
			states.push(tab.ownership._tag);
			if (tab.edit_state._tag === "Dirty") {
				states.push("Dirty");
			}
			if (Option.isSome(tab.agent_change)) {
				states.push(`Agent change · ${tab.agent_change.value.agent_name}`);
			}

			return `${tab.file.name} · ${states.join(" · ")}`;
		});

	const TabOptions = (tabs: ReadonlyArray<WorkspaceTab>) =>
		Effect.gen(function* () {
			const options: Array<{ id: string; label: string }> = [];

			for (const tab of tabs) {
				options.push({ id: tab.id, label: yield* TabOptionLabel(tab) });
			}

			return options;
		});

	let overflow_tab_options = $derived(yield* TabOptions(overflow_tabs));
</script>

<nav class="workspace-navigation" aria-label="Workspace file navigation">
	<div class="breadcrumbs" aria-label="Active file breadcrumbs">
		<FolderCode size={13} stroke={1.7} aria-hidden="true" />
		{#each breadcrumbs as segment, index}
			{#if index > 0}<ChevronRight size={11} stroke={1.6} aria-hidden="true" />{/if}
			<span class:last={index === breadcrumbs.length - 1}>{segment}</span>
		{/each}
	</div>
	<div class="navigation-selects">
		<label>
			<span class="sr-only">Recent files</span>
			<select aria-label="Recent files" bind:value={recent_selection} onchange={yield* SelectRecent(event.currentTarget.value)}>
				<option value="">Recent</option>
				{#each recent_files as file}<option value={file.id}>{file.name}</option>{/each}
			</select>
		</label>
		<label>
			<span class="sr-only">Changed files</span>
			<select aria-label="Changed files" bind:value={changed_selection} onchange={yield* SelectChanged(event.currentTarget.value)}>
				<option value="">Changed ({changed_files.length})</option>
				{#each changed_files as changed}<option value={changed.file.id}>{changed.file.name} · {changed.change.agent_name}</option>{/each}
			</select>
		</label>
		{#if overflow_tabs.length > 0}
			<label>
				<span class="sr-only">Overflow tabs</span>
				<select aria-label="Overflow tabs" bind:value={overflow_selection} onchange={yield* SelectOverflow(event.currentTarget.value)}>
					<option value="">More tabs ({overflow_tabs.length})</option>
					{#each overflow_tab_options as option}<option value={option.id}>{option.label}</option>{/each}
				</select>
			</label>
		{/if}
	</div>
</nav>

<style>
	.workspace-navigation {
		display: flex;
		min-height: 30px;
		align-items: center;
		justify-content: space-between;
		gap: 10px;
		padding: 0 8px 0 12px;
		border-bottom: 1px solid var(--line);
		background: color-mix(in oklch, var(--pane) 82%, var(--pane-inset));
	}

	.breadcrumbs {
		display: flex;
		min-width: 0;
		align-items: center;
		gap: 3px;
		color: var(--text-muted);
		font-size: 10px;
	}

	.breadcrumbs span {
		overflow: hidden;
		max-width: 110px;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.breadcrumbs span.last {
		color: var(--text-secondary);
	}

	.navigation-selects {
		display: flex;
		flex: 0 0 auto;
		align-items: center;
		gap: 4px;
	}

	select {
		max-width: 122px;
		height: 22px;
		padding: 0 20px 0 7px;
		border: 1px solid var(--line);
		border-radius: var(--radius-sm);
		background: var(--pane-inset);
		color: var(--text-muted);
		font-size: 9px;
		cursor: pointer;
	}

	.sr-only {
		position: absolute;
		width: 1px;
		height: 1px;
		padding: 0;
		margin: -1px;
		overflow: hidden;
		clip: rect(0, 0, 0, 0);
		white-space: nowrap;
		border: 0;
	}

	@media (max-width: 799px) {
		.workspace-navigation {
			align-items: flex-start;
			flex-direction: column;
			padding: 6px 8px;
		}

		.navigation-selects {
			width: 100%;
			overflow-x: auto;
		}
	}
</style>
