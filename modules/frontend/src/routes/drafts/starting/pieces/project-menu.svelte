<script lang="ts">
	/**
	 * The project picker as a menu. The variants that keep today's layout need
	 * the choice to cost no vertical space at all, and a menu is the only shape
	 * that costs none until it is asked for.
	 *
	 * It grows from its trigger rather than from its own centre, and the
	 * backdrop that dismisses it is a real element so a click anywhere lands
	 * somewhere deliberate.
	 */
	import Check from "@tabler/icons-svelte/icons/check";
	import FolderPlus from "@tabler/icons-svelte/icons/folder-plus";
	import type { Snippet } from "svelte";
	import { DraftProjects, type DraftProject } from "../mock";
	import Monogram from "./monogram.svelte";
	import RepoState from "./repo-state.svelte";

	let {
		class: klass = "",
		onselect,
		origin = "origin-bottom-left",
		placement = "above",
		selected,
		trigger,
	}: {
		class?: string;
		onselect: (project: DraftProject) => void;
		origin?: string;
		placement?: "above" | "below";
		selected: DraftProject;
		trigger: Snippet<[{ open: boolean; project: DraftProject }]>;
	} = $props();

	let open = $state(false);
</script>

<div class={`relative ${klass}`}>
	<button
		aria-expanded={open}
		class="dz-press dz-focus block rounded-lg outline-none"
		onclick={() => (open = !open)}
		type="button"
	>
		{@render trigger({ open, project: selected })}
	</button>

	{#if open}
		<button
			aria-label="Dismiss"
			class="fixed inset-0 z-10 cursor-default"
			onclick={() => (open = false)}
			type="button"
		></button>
		<div
			class={`dz-pop card-lg absolute z-20 w-[22rem] rounded-xl border border-border bg-surface-900 p-1.5 ${origin} ${placement === "above" ? "bottom-full mb-2" : "top-full mt-2"} left-0`}
		>
			{#each DraftProjects as project (project.project_id)}
				<button
					class="dz-row dz-press-row dz-focus flex w-full items-center gap-2.5 rounded-lg px-2 py-1.5 text-left outline-none"
					onclick={() => {
						onselect(project);
						open = false;
					}}
					type="button"
				>
					<Monogram class="size-7 rounded-md text-[9px]" {project} />
					<span class="min-w-0 flex-1">
						<span class="block truncate text-sm text-foreground">{project.name}</span>
						<RepoState {project} />
					</span>
					{#if project.project_id === selected.project_id}
						<Check class="size-4 shrink-0 text-foreground-extra" />
					{/if}
				</button>
			{/each}
			<div class="my-1 h-px bg-border"></div>
			<button
				class="dz-row dz-press-row dz-focus flex w-full items-center gap-2.5 rounded-lg px-2 py-2 text-left text-sm text-muted-foreground outline-none"
				type="button"
			>
				<FolderPlus class="size-4 shrink-0" />
				Attach a folder…
			</button>
		</div>
	{/if}
</div>
