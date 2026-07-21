<script lang="ts" effect>
	import { Effect, Option } from "effect";
	import { IconChevronRight as ChevronRight, IconFolderCode as FolderCode } from "@tabler/icons-svelte";

	import type {
		ChangedFile,
		WorkspaceFileReference,
		WorkspaceTab,
	} from "$lib/workspace/workspace-tab-model";
	import { Button } from "$lib/components/ui/button";
	import {
		DropdownMenu,
		DropdownMenuContent,
		DropdownMenuItem,
		DropdownMenuTrigger,
	} from "$lib/components/ui/dropdown-menu";

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
		<DropdownMenu>
			<DropdownMenuTrigger>
				{#snippet child({ props })}<Button size="xs" variant="outline" aria-label="Recent files" {...props}>Recent</Button>{/snippet}
			</DropdownMenuTrigger>
			<DropdownMenuContent align="end">
				{#if recent_files.length === 0}<DropdownMenuItem disabled>No recent files</DropdownMenuItem>{/if}
				{#each recent_files as file}<DropdownMenuItem onclick={yield* on_open_recent(file.id)}>{file.name}</DropdownMenuItem>{/each}
			</DropdownMenuContent>
		</DropdownMenu>
		<DropdownMenu>
			<DropdownMenuTrigger>
				{#snippet child({ props })}<Button size="xs" variant="outline" aria-label="Changed files" {...props}>Changed ({changed_files.length})</Button>{/snippet}
			</DropdownMenuTrigger>
			<DropdownMenuContent align="end">
				{#if changed_files.length === 0}<DropdownMenuItem disabled>No changed files</DropdownMenuItem>{/if}
				{#each changed_files as changed}<DropdownMenuItem onclick={yield* on_open_changed(changed.file.id)}>{changed.file.name} · {changed.change.agent_name}</DropdownMenuItem>{/each}
			</DropdownMenuContent>
		</DropdownMenu>
		{#if overflow_tabs.length > 0}
			<DropdownMenu>
				<DropdownMenuTrigger>
					{#snippet child({ props })}<Button size="xs" variant="outline" aria-label="Overflow tabs" {...props}>More tabs ({overflow_tabs.length})</Button>{/snippet}
				</DropdownMenuTrigger>
				<DropdownMenuContent align="end">
					{#each overflow_tabs as tab}<DropdownMenuItem onclick={yield* on_activate_overflow(tab.id)}>{yield* TabOptionLabel(tab)}</DropdownMenuItem>{/each}
				</DropdownMenuContent>
			</DropdownMenu>
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
