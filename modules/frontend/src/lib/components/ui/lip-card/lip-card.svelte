<script lang="ts">
	import type { Snippet } from "svelte";
	import type { HTMLAttributes } from "svelte/elements";

	import { cn, type WithElementRef } from "$lib/utils.js";

	let {
		ref = $bindable(null),
		class: className,
		children,
		lip,
		open = false,
		...restProps
	}: WithElementRef<HTMLAttributes<HTMLDivElement>> & {
		children: Snippet;
		lip: Snippet;
		open?: boolean;
	} = $props();
</script>

<div
	bind:this={ref}
	data-slot="lip-card"
	data-open={open}
	class={cn(
		"t-acc flex flex-col overflow-hidden bg-linear-to-b from-surface-200 to-surface-125 dark:from-surface-850 dark:to-surface-900 card",
		className,
	)}
	{...restProps}
>
	<div class="relative z-10 w-full">
		{@render children()}
	</div>
	<div class="t-acc-panel">
		<div class="t-acc-panel-inner">
			{@render lip()}
		</div>
	</div>
</div>

<style>
	/* grid-template-rows 0fr → 1fr gives a clean height animation
	   with no JS measurement; the inner element clips overflow. */
	.t-acc-panel {
		display: grid;
		grid-template-rows: 0fr;
		pointer-events: none;
		transition: grid-template-rows var(--acc-collapse) var(--acc-ease);
	}

	.t-acc[data-open="true"] .t-acc-panel {
		grid-template-rows: 1fr;
		pointer-events: auto;
		transition: grid-template-rows var(--acc-expand) var(--acc-ease);
	}

	.t-acc-panel-inner {
		min-height: 0;
		overflow: hidden;
		opacity: 0;
		filter: blur(2px);
		transition:
			opacity var(--acc-collapse) var(--acc-ease),
			filter var(--acc-collapse) var(--acc-ease);
	}

	.t-acc[data-open="true"] .t-acc-panel-inner {
		opacity: 1;
		filter: blur(0);
		transition:
			opacity var(--acc-expand) var(--acc-ease),
			filter var(--acc-expand) var(--acc-ease);
	}

	@media (prefers-reduced-motion: reduce) {
		.t-acc-panel,
		.t-acc-panel-inner {
			transition: none !important;
		}
	}
</style>
