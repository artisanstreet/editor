<script lang="ts">
	/**
	 * Twenty-eight days of a project, as bars. A dead day still draws a stub so
	 * the baseline stays a line rather than a gap — an absence you can see is
	 * information, an absence that collapses the row is noise.
	 */
	let {
		class: klass = "h-6",
		tone = "var(--surface-400)",
		values,
	}: {
		class?: string;
		tone?: string;
		values: ReadonlyArray<number>;
	} = $props();
</script>

<div aria-hidden="true" class={`flex w-full items-end gap-px ${klass}`}>
	{#each values as value, index (index)}
		<div
			class="min-w-px flex-1 rounded-[1px]"
			style:background={tone}
			style:height={`${Math.max(8, value * 100)}%`}
			style:opacity={0.22 + value * 0.78}
		></div>
	{/each}
</div>
