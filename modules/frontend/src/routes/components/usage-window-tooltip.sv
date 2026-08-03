<script lang="ts">
	/**
	 * Presentational only. The numbers are tweened by the panel rather than by this
	 * component, so moving between rows carries one value onto the next instead of
	 * restarting every time a tooltip mounts.
	 */
	let {
		remaining,
	}: {
		readonly remaining: number;
	} = $props();
</script>

<span class="text-muted-foreground">
	You have <span class="tabular-nums text-foreground">{Math.round(remaining)}%</span> left.
</span>
