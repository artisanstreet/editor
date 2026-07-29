<script lang="ts">
	import type { Snippet } from "svelte";
	import type { HTMLAttributes } from "svelte/elements";
	import { cn, type WithElementRef } from "$lib/utils";
	import PaperGodRays from "./paper-god-rays.sv";

	let {
		ref = $bindable(null),
		class: class_name,
		children,
		strength = "quiet",
		use_card = true,
		use_rays = true,
		...rest_props
	}: WithElementRef<HTMLAttributes<HTMLDivElement>> & {
		children: Snippet;
		strength?: "quiet" | "strong";
		use_card?: boolean;
		use_rays?: boolean;
	} = $props();
</script>

<div
	bind:this={ref}
	class={cn(
		"shader-glass-surface relative isolate overflow-hidden",
		use_card && "card-glass",
		class_name,
	)}
	data-strength={strength}
	{...rest_props}
>
	{#if use_rays}
		<div aria-hidden="true" class="pointer-events-none absolute inset-0 z-0 overflow-hidden">
			<PaperGodRays />
		</div>
	{/if}
	<div aria-hidden="true" class="shader-glass-material"></div>
	<div aria-hidden="true" class="shader-glass-highlight"></div>
	<div class="relative z-10 size-full">
		{@render children()}
	</div>
</div>

<style>
	.shader-glass-material {
		position: absolute;
		inset: 0;
		z-index: 1;
		pointer-events: none;
		/**
		 * Chromium composites backdrop-filter without honoring the ancestor's
		 * rounded overflow clip; the layer must carry the radius itself or the
		 * blurred square corners bleed past the surface.
		 */
		border-radius: inherit;
		background: linear-gradient(145deg, rgb(82 82 91 / 0.2), rgb(39 39 42 / 0.14));
		-webkit-backdrop-filter: blur(12px) saturate(115%) brightness(102%);
		backdrop-filter: blur(12px) saturate(115%) brightness(102%);
	}

	.shader-glass-highlight {
		position: absolute;
		inset: 0;
		z-index: 2;
		pointer-events: none;
		background: linear-gradient(180deg, rgb(255 255 255 / 0.05), transparent 42%);
	}

	.shader-glass-surface[data-strength="strong"] .shader-glass-material {
		background: linear-gradient(145deg, rgb(82 82 91 / 0.28), rgb(39 39 42 / 0.2));
		-webkit-backdrop-filter: blur(20px) saturate(120%) brightness(103%);
		backdrop-filter: blur(20px) saturate(120%) brightness(103%);
	}

	.shader-glass-surface[data-strength="strong"] .shader-glass-highlight {
		background: linear-gradient(180deg, rgb(255 255 255 / 0.08), transparent 44%);
	}

	@supports not ((-webkit-backdrop-filter: blur(1px)) or (backdrop-filter: blur(1px))) {
		.shader-glass-material {
			background: var(--surface-825);
		}
	}

	@media (prefers-reduced-transparency: reduce) {
		.shader-glass-material {
			background: var(--surface-825);
			-webkit-backdrop-filter: none;
			backdrop-filter: none;
		}
	}
</style>
