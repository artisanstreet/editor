<script lang="ts">
	import type { Snippet } from "svelte";
	import type { HTMLAttributes } from "svelte/elements";
	import { shader_enabled } from "$lib/appearance-config";
	import { cn, type WithElementRef } from "$lib/utils";
	import PaperGodRays from "./paper-god-rays.svelte";

	let {
		ref = $bindable(null),
		class: class_name,
		children,
		content_column = false,
		strength = "quiet",
		use_card = true,
		use_rays = true,
		use_material = true,
		...rest_props
	}: WithElementRef<HTMLAttributes<HTMLDivElement>> & {
		children: Snippet;
		/**
		 * Makes the content wrapper a shrinkable flex column so a height bound on
		 * the card actually reaches a scroll box inside it. The default block
		 * wrapper swallows `min-h-0`, which lets tall content grow past the card
		 * and clip against its overflow instead of scrolling.
		 */
		content_column?: boolean;
		strength?: "quiet" | "strong";
		use_card?: boolean;
		use_rays?: boolean;
		/**
		 * Off leaves the surface unglazed: the rays still light it, but the frosted
		 * material belongs to a nested surface instead of covering the whole card.
		 */
		use_material?: boolean;
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
	<!--
		The preference gates the light, never the glass: with the shader off the
		surface keeps its material, highlight, and card so the layout and contrast
		are unchanged and only the animation stops.
	-->
	{#if use_rays && $shader_enabled}
		<div aria-hidden="true" class="pointer-events-none absolute inset-0 z-0 overflow-hidden">
			<PaperGodRays />
		</div>
	{/if}
	{#if use_material}
		<div aria-hidden="true" class="shader-glass-material"></div>
		<div aria-hidden="true" class="shader-glass-highlight"></div>
	{/if}
	<div class={content_column ? "relative z-10 flex size-full min-h-0 flex-col" : "relative z-10 size-full"}>
		{@render children()}
	</div>
</div>

