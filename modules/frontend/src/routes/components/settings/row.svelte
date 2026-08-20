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

<div class="flex flex-col items-stretch gap-3 px-4 py-3.5 sm:flex-row sm:items-center sm:justify-between sm:gap-6">
	<div class="flex min-w-0 flex-col gap-0.5">
		<span class="text-sm text-foreground">{title}</span>
		<span class="text-pretty text-xs leading-relaxed text-muted-foreground">{description}</span>
	</div>
	{#if control !== undefined}
		<div class="flex w-full shrink-0 justify-end sm:w-auto">
			{@render control()}
		</div>
	{/if}
</div>
