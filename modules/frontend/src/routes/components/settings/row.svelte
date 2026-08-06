<script lang="ts">
	import type { Snippet } from "svelte";

	let {
		control,
		description,
		title,
	}: {
		control?: Snippet;
		description: string;
		title: string;
	} = $props();
</script>

<div class="flex items-center justify-between gap-6 px-4 py-3.5">
	<div class="flex min-w-0 flex-col gap-0.5">
		<span class="text-sm text-foreground">{title}</span>
		<span class="text-pretty text-xs text-muted-foreground">{description}</span>
	</div>
	{#if control !== undefined}
		{@render control()}
	{/if}
</div>
