<script lang="ts">
	import type { Snippet } from "svelte";

	let {
		children,
		class: class_name = "",
	}: {
		children: Snippet;
		class?: string;
	} = $props();
</script>

<!--
	The settings surface: every group of rows sits in the same moulded gradient
	well the app's opaque controls use, with hairline dividers between rows.
-->
<div
	class={`card flex flex-col divide-y divide-border/40 overflow-hidden rounded-xl bg-linear-to-b from-surface-225 to-surface-200 dark:from-surface-800 dark:to-surface-925 ${class_name}`}
>
	{@render children()}
</div>
