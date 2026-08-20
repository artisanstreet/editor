<script lang="ts">
	/**
	 * What the repository is doing, in one line. A clean tree says so in words
	 * rather than by omission: "nothing" and "we did not check" look identical
	 * when the answer is an empty space.
	 */
	import GitBranch from "@tabler/icons-svelte/icons/git-branch";
	import type { DraftProject } from "../mock";

	let {
		class: klass = "",
		project,
	}: { class?: string; project: DraftProject } = $props();
</script>

<span
	class={`flex min-w-0 items-center gap-1.5 text-xs text-muted-foreground ${klass}`}
>
	<GitBranch class="size-3.5 shrink-0" />
	<span class="truncate font-mono">{project.branch}</span>
	{#if project.dirty > 0}
		<span class="text-warning">·&nbsp;{project.dirty} changed</span>
	{:else}
		<span class="opacity-60">·&nbsp;clean</span>
	{/if}
	{#if project.ahead > 0}
		<span class="opacity-60">·&nbsp;↑{project.ahead}</span>
	{/if}
</span>
