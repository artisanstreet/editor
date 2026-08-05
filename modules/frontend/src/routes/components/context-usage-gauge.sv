<script lang="ts" effect>
	import { page } from "$app/state";
	import { Effect } from "effect";
	import type { SurfaceUsageAggregate, ThreadUsageSeries } from "@artisan/protocol";
	import { ArtisanClient } from "@artisan/transport/client";
	import { LinkPreview } from "bits-ui";
	import { ContextUsageDescription } from "$lib/context-usage/description";
	import ContextUsageDetails from "./context-usage-details.sv";
	import ContextUsageRing from "./context-usage-ring.sv";
	import ShaderGlassSurface from "./shader-glass-surface.sv";

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

	const client = yield* ArtisanClient;
	let series = $state.raw<ThreadUsageSeries | undefined>(undefined);

	/**
	 * Re-read whenever the reported context total moves, which is once per turn:
	 * the series is the same fact at finer grain, so anything that changes one
	 * changes the other. Keyed on the thread so a navigation never paints the
	 * previous thread's history.
	 */
	const LoadSeries = (thread_id: string | undefined, _revision: number | undefined) =>
		Effect.gen(function* () {
			if (thread_id === undefined) {
				series = undefined;
				return;
			}
			series = yield* client.GetThreadUsageSeries({ thread_id });
		}).pipe(
			/** A missing series costs the chart, not the reading above it. */
			Effect.catch(() =>
				Effect.gen(function* () {
					series = undefined;
				}),
			),
		);
	yield* LoadSeries(page.params.thread, usage.context_tokens);
</script>

<!--
	A hover card rather than a tooltip. A tooltip is a non-interactive aside and
	dismisses when the pointer leaves its trigger, so the chart's own points —
	which exist to be hovered for a per-turn reading — could never be reached.
	This surface holds while the pointer is inside it, which is what makes the
	points reachable at all.
-->
<LinkPreview.Root openDelay={0} closeDelay={120}>
	<LinkPreview.Trigger>
		{#snippet child({ props: preview_props })}
			<button
				type="button"
				{...preview_props}
				class="context-gauge-trigger flex size-6 shrink-0 cursor-default items-center justify-center rounded-md outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50 focus-visible:ring-inset"
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
		stripped so the surface is the only thing painted.
	-->
	<LinkPreview.Portal>
		<LinkPreview.Content
			side="top"
			align="start"
			sideOffset={8}
			class="z-50 block w-72 max-w-[min(20rem,calc(100vw-2rem))] rounded-2xl bg-transparent p-0 text-foreground shadow-none outline-none"
		>
			<ShaderGlassSurface class="w-full rounded-2xl">
				<ContextUsageDetails {model_name} {percent} {series} {window_tokens} />
			</ShaderGlassSurface>
		</LinkPreview.Content>
	</LinkPreview.Portal>
</LinkPreview.Root>

<style>
	/**
	 * The housing responds to hover, the mark does not: the arc's colour is the
	 * reading, so tinting it would say the window changed when only the pointer
	 * did.
	 *
	 * Sized close to the mark rather than to the row: the picker already carries
	 * its own inset, so a full-height well here stacked two paddings and a gap
	 * into a gulf between the two. The well stays a rounded square like every
	 * other control on this row — the dial is the only circle, and a circular
	 * housing around it would make the row read as two vocabularies.
	 */
	.context-gauge-trigger {
		background-color: transparent;
		transition: background-color var(--duration-fast) var(--ease-in-out);
	}

	.context-gauge-trigger:hover,
	.context-gauge-trigger:focus-visible {
		background-color: color-mix(in oklch, var(--foreground) 6%, transparent);
	}

	@media (prefers-reduced-motion: reduce) {
		.context-gauge-trigger {
			transition: none;
		}
	}
</style>
