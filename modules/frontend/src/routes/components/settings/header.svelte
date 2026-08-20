<script lang="ts">
	import type { Snippet } from "svelte";

	let {
		description,
		mark,
		title,
	}: {
		description: string;
		/** An optional leading emblem — engine pages put their engine mark here. */
		mark?: Snippet;
		title: string;
	} = $props();
</script>

<header>
	<h1 class="flex items-center gap-2.5 text-xl font-semibold tracking-tight text-foreground">
		{#if mark !== undefined}
			{@render mark()}
		{/if}
		{title}
	</h1>
	<p class="mt-1.5 text-sm text-pretty text-muted-foreground">{description}</p>
</header>
