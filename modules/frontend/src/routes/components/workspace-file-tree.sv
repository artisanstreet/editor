<script lang="ts">
	import ChevronRight from "@tabler/icons-svelte/icons/chevron-right";
	import Folder from "@tabler/icons-svelte/icons/folder";
	import FolderOpen from "@tabler/icons-svelte/icons/folder-open";

	import { resolve_file_icon } from "$lib/conversation/file-icon";

	import type { WorkspaceTreeEntry } from "$lib/editor/workspace-session";
	import { workspace_tree_root } from "$lib/editor/workspace-session";
	import Self from "./workspace-file-tree.sv";

	/**
	 * One level of the workspace tree.
	 *
	 * Directories start closed and their children are fetched the first time one
	 * is opened, so mounting the tree costs a single top-level listing however
	 * large the repository is.
	 */
	let {
		active_path,
		children_by_path,
		depth = 0,
		expanded,
		onopen,
		ontoggle,
		parent = workspace_tree_root,
	}: {
		readonly active_path?: string;
		readonly children_by_path: ReadonlyMap<string, ReadonlyArray<WorkspaceTreeEntry>>;
		readonly depth?: number;
		readonly expanded: ReadonlySet<string>;
		readonly onopen: (path: string) => void;
		readonly ontoggle: (path: string) => void;
		readonly parent?: string;
	} = $props();

	const entries = $derived(children_by_path.get(parent) ?? []);
	/** Indentation is the only thing depth is for; the guide rail sits at the row's left edge. */
	const indent = (extra: number) => `padding-left: ${0.25 + depth * 0.75 + extra}rem`;
</script>

{#each entries as entry (entry.path)}
	{#if entry.kind === "directory"}
		{@const is_expanded = expanded.has(entry.path)}
		<button
			type="button"
			class="flex w-full min-w-0 items-center gap-1.5 rounded-lg py-1 pr-2 text-left text-sm text-muted-foreground transition-colors hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none"
			style={indent(0)}
			aria-expanded={is_expanded}
			onclick={() => ontoggle(entry.path)}
		>
			<ChevronRight
				class={`size-3.5 shrink-0 transition-transform duration-(--duration-fast) ease-in-out motion-reduce:transition-none ${
					is_expanded ? "rotate-90" : ""
				}`}
			/>
			{#if is_expanded}
				<FolderOpen class="size-3.5 shrink-0" />
			{:else}
				<Folder class="size-3.5 shrink-0" />
			{/if}
			<span class="truncate">{entry.name}</span>
		</button>
		{#if is_expanded}
			{#if children_by_path.has(entry.path)}
				<Self
					{active_path}
					{children_by_path}
					depth={depth + 1}
					{expanded}
					{onopen}
					{ontoggle}
					parent={entry.path}
				/>
			{:else}
				<p class="py-1 text-xs text-muted-foreground" style={indent(1.5)}>Loading…</p>
			{/if}
		{/if}
	{:else}
		<button
			type="button"
			class="flex w-full min-w-0 items-center gap-1.5 rounded-lg py-1 pr-2 text-left text-sm text-muted-foreground transition-colors hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none aria-current:text-foreground"
			style={indent(0.75)}
			aria-current={entry.path === active_path ? "true" : undefined}
			onclick={() => onopen(entry.path)}
		>
			<img
				src={resolve_file_icon(entry.path)}
				alt=""
				aria-hidden="true"
				class="size-4 shrink-0"
			/>
			<span class="truncate">{entry.name}</span>
		</button>
	{/if}
{/each}
