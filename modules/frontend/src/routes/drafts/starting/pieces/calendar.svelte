<script lang="ts">
	/**
	 * The activity grid the landing page already wears, reduced to what the
	 * drafts need from it: a block of days whose density says how busy a stretch
	 * of time was. Columns are weeks, rows are weekdays.
	 */
	let {
		class: klass = "",
		tone = "var(--foreground)",
		values,
	}: { class?: string; tone?: string; values: ReadonlyArray<number> } = $props();
</script>

<div
	aria-hidden="true"
	class={`grid grid-flow-col grid-rows-7 gap-[3px] ${klass}`}
>
	{#each values as value, index (index)}
		<div
			class="size-2.5 rounded-[2px]"
			style:background={tone}
			style:opacity={value === 0 ? 0.06 : 0.14 + value * 0.7}
		></div>
	{/each}
</div>
