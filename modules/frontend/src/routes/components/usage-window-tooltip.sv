<script lang="ts">
	import type { ResetParts } from "$lib/identity/usage-window-motion";

	/**
	 * Presentational only. The numbers are tweened by the panel rather than by this
	 * component, so moving between rows carries one value onto the next instead of
	 * restarting every time a tooltip mounts.
	 */
	let {
		amount,
		remaining,
		reset,
	}: {
		readonly amount: number;
		readonly remaining: number;
		readonly reset?: ResetParts;
	} = $props();
</script>

<span>
	{#if reset !== undefined}
		{reset.past ? "Reset" : "Resets in"}
		<span class="tabular-nums">{Math.round(amount)}{reset.unit}</span>{reset.past
			? " ago"
			: ""}. You have <span class="tabular-nums">{Math.round(remaining)}%</span> left.
	{:else}
		You have <span class="tabular-nums">{Math.round(remaining)}%</span> left.
	{/if}
</span>
