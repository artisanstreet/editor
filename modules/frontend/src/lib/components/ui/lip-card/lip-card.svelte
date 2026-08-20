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
		animate = true,
		variant = "solid",
		...restProps
	}: WithElementRef<HTMLAttributes<HTMLDivElement>> & {
		children: Snippet;
		lip: Snippet;
		open?: boolean;
		/**
		 * `false` paints open-state changes instantly. Hold it false while the
		 * mount-time state is still settling (e.g. async data deciding `open`),
		 * then flip it once that state is on screen so only real changes animate.
		 */
		animate?: boolean;
		/** `glass` drops the card's own fill and shadow: the surface behind it supplies both. */
		variant?: "solid" | "glass";
	} = $props();
</script>

<div
	bind:this={ref}
	data-slot="lip-card"
	data-open={open}
	data-animate={animate}
	class={cn(
		"t-acc flex flex-col overflow-hidden",
		variant === "solid" &&
			"bg-linear-to-b from-surface-200 to-surface-125 dark:from-surface-850 dark:to-surface-900 card",
		className,
	)}
	{...restProps}
>
	<div class="relative z-10 w-full">
		{@render children()}
	</div>
	<!--
		`inert` rather than only `pointer-events: none`: the panel collapses to zero
		height while staying in the layout, so a control inside it would otherwise
		remain focusable with nothing visible on screen.
	-->
	<div class="t-acc-panel" inert={!open}>
		<div class="t-acc-panel-inner">
			{@render lip()}
		</div>
	</div>
</div>

