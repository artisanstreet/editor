<script lang="ts" module>
	export type ShimmerTextVariant =
		| "default"
		| "secondary"
		| "destructive"
		| "red"
		| "blue"
		| "green"
		| "yellow"
		| "purple"
		| "pink"
		| "orange"
		| "cyan"
		| "indigo"
		| "violet"
		| "rose"
		| "amber"
		| "lime"
		| "emerald"
		| "sky"
		| "slate"
		| "fuchsia";

	const variant_classes: Record<ShimmerTextVariant, string> = {
		amber: "text-amber-600 dark:text-amber-400",
		blue: "text-blue-600 dark:text-blue-400",
		cyan: "text-cyan-600 dark:text-cyan-400",
		default: "text-foreground",
		destructive: "text-destructive",
		emerald: "text-emerald-600 dark:text-emerald-400",
		fuchsia: "text-fuchsia-600 dark:text-fuchsia-400",
		green: "text-green-600 dark:text-green-400",
		indigo: "text-indigo-600 dark:text-indigo-400",
		lime: "text-lime-600 dark:text-lime-400",
		orange: "text-orange-600 dark:text-orange-400",
		pink: "text-pink-600 dark:text-pink-400",
		purple: "text-purple-600 dark:text-purple-400",
		red: "text-red-600 dark:text-red-400",
		rose: "text-rose-600 dark:text-rose-400",
		secondary: "text-secondary-foreground",
		sky: "text-sky-600 dark:text-sky-400",
		slate: "text-slate-600 dark:text-slate-400",
		violet: "text-violet-600 dark:text-violet-400",
		yellow: "text-yellow-600 dark:text-yellow-400",
	};
</script>

<script lang="ts">
	import type { Snippet } from "svelte";
	import type { HTMLSpanAttributes } from "svelte/elements";
	import { cn, type WithElementRef } from "$lib/utils.js";

	let {
		active = true,
		ref = $bindable(null),
		children,
		class: class_name,
		delay = 1.5,
		duration = 3,
		spread = 50,
		style,
		variant = "default",
		...rest_props
	}: WithElementRef<HTMLSpanAttributes> & {
		/**
		 * Whether the sweep is running. A caller whose text shimmers only some of
		 * the time must be able to say so here rather than swapping this component
		 * for a plain span: that swap replaces the subtree, and any entrance the
		 * text inside it owns replays from nothing on every toggle.
		 */
		active?: boolean;
		children?: Snippet;
		delay?: number;
		duration?: number;
		spread?: number;
		variant?: ShimmerTextVariant;
	} = $props();

	const shimmer_style = $derived(
		`${style ?? ""};--shimmer-delay:${delay}s;--shimmer-duration:${duration}s;--shimmer-spread:${spread}%`,
	);
</script>

<span
	bind:this={ref}
	data-slot="shimmer-text"
	class={cn(
		"inline-block",
		active && "t-shimmer-text",
		variant_classes[variant],
		class_name,
	)}
	style={shimmer_style}
	{...rest_props}
>
	{@render children?.()}
</span>

<style>
	.t-shimmer-text {
		--shimmer-contrast: color-mix(in srgb, currentColor 35%, white);
		background-color: currentColor;
		background-image: linear-gradient(
			to right,
			currentColor 0%,
			var(--shimmer-contrast) 40%,
			var(--shimmer-contrast) 60%,
			currentColor 100%
		);
		background-position-x: 250%;
		background-repeat: no-repeat;
		background-size: var(--shimmer-spread) 200%;
		background-clip: text;
		-webkit-background-clip: text;
		-webkit-text-fill-color: transparent;
		animation: shimmer-text var(--shimmer-duration) linear var(--shimmer-delay) infinite;
	}

	:global(.dark) .t-shimmer-text {
		--shimmer-contrast: color-mix(in srgb, currentColor 45%, black);
	}

	@keyframes shimmer-text {
		from {
			background-position-x: 250%;
		}
		to {
			background-position-x: -100%;
		}
	}

	@media (prefers-reduced-motion: reduce) {
		.t-shimmer-text {
			animation: none !important;
			background-image: none;
			color: currentColor;
			-webkit-text-fill-color: currentColor;
		}
	}
</style>
