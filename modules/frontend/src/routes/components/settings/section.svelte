<script lang="ts">
	import type { Snippet } from "svelte";

	let {
		action,
		children,
		id,
		intro,
		title,
	}: {
		/** A control sitting on the header's baseline — "Refresh", "Restore defaults". */
		action?: Snippet;
		children: Snippet;
		id: string;
		intro?: string;
		title: string;
	} = $props();
</script>

<section class="mt-10 sm:mt-12" aria-labelledby={id}>
	<div class="flex items-center justify-between gap-4">
		<h2 {id} class="scroll-mt-28 text-sm font-medium text-foreground md:scroll-mt-6">{title}</h2>
		{#if action !== undefined}
			{@render action()}
		{/if}
	</div>
	{#if intro !== undefined}
		<p class="mt-1 text-sm text-pretty text-muted-foreground">{intro}</p>
	{/if}
	{@render children()}
</section>
