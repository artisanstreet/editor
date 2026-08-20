<script lang="ts">
	import type { SurfaceUsageAggregate } from "@artisan/protocol";
	import { LinkPreview } from "bits-ui";
	import { ContextUsageDescription } from "$lib/context-usage/description";
	import ContextUsageDetails from "./context-usage-details.svelte";
	import ContextUsageRing from "./context-usage-ring.svelte";
	import ShaderGlassSurface from "./shader-glass-surface.svelte";

	/**
	 * The context-window gauge as its own control, standing beside the model
	 * picker rather than inside it. Being a sibling is what lets it own a
	 * tooltip again: nested inside the picker's trigger it could only be a mark,
	 * because a button within a button is invalid and its hover would have been
	 * taken by the pill around it.
	 */
	let {
		compaction_percent,
		model_name,
		percent,
		usage,
		window_tokens,
	}: {
		/** Where this harness and model begin compacting; the red leg ends there. */
		readonly compaction_percent: number;
		/** Named on the card, because a window size only means something per model. */
		readonly model_name?: string;
		readonly percent: number;
		readonly usage: SurfaceUsageAggregate;
		readonly window_tokens: number;
	} = $props();

	const description = $derived(ContextUsageDescription(usage, window_tokens));
</script>

<!--
	A hover card rather than a tooltip so the reading holds while the pointer
	moves into it, and so every floating surface on this row opens with the same
	timing and material.
-->
<LinkPreview.Root openDelay={0} closeDelay={120}>
	<LinkPreview.Trigger>
		{#snippet child({ props: preview_props })}
			<button
				type="button"
				{...preview_props}
				class="flex size-6 shrink-0 cursor-default items-center justify-center rounded-md bg-transparent outline-none transition-colors duration-(--duration-fast) ease-in-out hover:bg-[color-mix(in_oklch,var(--foreground)_6%,transparent)] focus-visible:bg-[color-mix(in_oklch,var(--foreground)_6%,transparent)] focus-visible:ring-[3px] focus-visible:ring-ring/50 focus-visible:ring-inset"
				aria-label={`Context window ${Math.round(percent)}% full`}
				aria-describedby="context-usage-details"
			>
				<ContextUsageRing {compaction_percent} {percent} />
			</button>
		{/snippet}
	</LinkPreview.Trigger>
	<!--
		Present whether or not the card is open: a focused trigger has to announce
		the reading, and the card's content does not exist until shown.
	-->
	<span id="context-usage-details" class="sr-only">{description}</span>
	<!--
		The same glass the account menu wears, so every floating surface in the
		app is one material. The primitive's own fill, padding and ring are
		stripped so the surface is the only thing painted. It enters with the
		tooltip's fade-and-scale, growing from the corner the positioning
		middleware names, so it arrives from its trigger like every other
		floating surface on this row.
	-->
	<LinkPreview.Portal>
		<LinkPreview.Content
			side="top"
			align="start"
			sideOffset={8}
			class="t-tt-presence z-50 block w-72 max-w-[min(20rem,calc(100vw-2rem))] origin-(--bits-link-preview-content-transform-origin) rounded-2xl bg-transparent p-0 text-foreground shadow-none outline-none"
		>
			<ShaderGlassSurface class="w-full rounded-2xl">
				<ContextUsageDetails {model_name} {percent} {window_tokens} />
			</ShaderGlassSurface>
		</LinkPreview.Content>
	</LinkPreview.Portal>
</LinkPreview.Root>

