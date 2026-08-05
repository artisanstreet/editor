<script lang="ts">
	import type { ThreadUsageSeries } from "@artisan/protocol";
	import DitherStackedArea from "$lib/components/dither/stacked-area.sv";
	import ShaderGlassSurface from "./shader-glass-surface.sv";

	/**
	 * The reading behind the gauge, as a card body.
	 *
	 * Deliberately not a by-category breakdown. Neither harness discloses one —
	 * both report totals only — and Artisan never holds the assembled prompt, so
	 * any "system prompt vs skills vs messages" split here would be invented.
	 * What is charted is what was actually reported, per turn.
	 */
	let {
		model_name,
		percent,
		series,
		window_tokens,
	}: {
		readonly model_name?: string;
		readonly percent: number;
		/** Absent until the series lands; the card renders its prose without it. */
		readonly series?: ThreadUsageSeries;
		readonly window_tokens: number;
	} = $props();

	/**
	 * Whole units only. A window size is a round capacity a reader recognises —
	 * "258K" is the fact; the ".4" is noise that reads as precision the number
	 * does not carry.
	 */
	const exact_tokens = new Intl.NumberFormat("en");
	const compact_tokens = new Intl.NumberFormat("en", {
		maximumFractionDigits: 0,
		notation: "compact",
	});

	const model = $derived(model_name ?? "this model");
	const points = $derived(series?.points ?? []);
	/**
	 * A single turn draws no shape worth reading, so the chart waits for a second
	 * point rather than rendering a flat stub.
	 */
	const has_chart = $derived(points.length > 1);
	/**
	 * How full the window was after each turn — the same reading the percentage
	 * above is taken from, at one point per turn.
	 *
	 * This deliberately does not chart the cached and uncached token totals. Those
	 * come from the provider's terminal usage frame, which accumulates across
	 * every model call in a turn, so on a chain of twenty tool calls they count
	 * the same context twenty times over. Cache reads dominate that sum so
	 * completely that the band rendered as solid cache at every scale — a "100%
	 * cached" reading that was really "this turn re-sent its context many times",
	 * drawn an order of magnitude above the window it sat under the heading of.
	 */
	const occupancy = $derived(points.map((point) => point.context_tokens ?? 0));
	const baseline = $derived(points.map(() => 0));
	/**
	 * A literal channel triple rather than a theme token: the dither is painted
	 * onto a canvas, which cannot resolve a CSS custom property.
	 */
	const occupancy_rgb = [139, 92, 246] as const;
	const percent_of_window = (tokens: number) =>
		window_tokens <= 0 ? 0 : Math.round((tokens / window_tokens) * 100);
</script>

<div class="flex w-full flex-col gap-3 p-4">
	<div class="flex min-w-0 flex-col gap-1">
		<span class="truncate text-sm font-semibold text-foreground">Context Window</span>
		<!--
			Two statements, two lines: the first is about this thread, the second
			about the model. Run together they read as one long sentence and the
			reader has to find where the subject changes.
		-->
		<span class="text-pretty text-xs leading-relaxed text-muted-foreground">
			The context window for {model} is
			<span class="tabular-nums text-foreground">{Math.round(percent)}%</span> full.
		</span>
		<span class="text-pretty text-xs leading-relaxed text-muted-foreground">
			{model} has a context window of
			<span class="tabular-nums text-foreground">{compact_tokens.format(window_tokens)}</span>
			tokens.
		</span>
	</div>

	<!--
		Named, because an unlabelled band is read as whatever the reader last knew
		it to be: this replaced a cached-over-fresh split, and without a caption a
		single colour goes on saying "all cache" rather than "this is occupancy".
	-->
	{#if has_chart}
		<span class="text-[0.6875rem] leading-none text-muted-foreground">
			Context used, per turn
		</span>
		<DitherStackedArea
			lower={occupancy}
			lower_color={occupancy_rgb}
			upper={baseline}
			upper_color={occupancy_rgb}
		>
			{#snippet tooltip({ index })}
				<!--
					The same glass the card itself wears, so a reading that opens over
					the chart reads as part of the surface rather than a second window
					floating on it.
				-->
				<ShaderGlassSurface class="rounded-xl" use_rays={false}>
					<div class="flex flex-col gap-1 px-2.5 py-2">
						<span class="whitespace-nowrap text-[0.6875rem] font-medium text-foreground">
							Turn {index + 1}
						</span>
						<span
							class="flex items-center gap-3 whitespace-nowrap text-[0.6875rem] text-muted-foreground"
						>
							Context
							<span class="ml-auto tabular-nums text-foreground">
								{exact_tokens.format(occupancy[index] ?? 0)}
							</span>
						</span>
						<span
							class="flex items-center gap-3 whitespace-nowrap text-[0.6875rem] text-muted-foreground"
						>
							Of window
							<span class="ml-auto tabular-nums text-foreground">
								{percent_of_window(occupancy[index] ?? 0)}%
							</span>
						</span>
					</div>
				</ShaderGlassSurface>
			{/snippet}
		</DitherStackedArea>
	{:else}
		<!--
			Said rather than left blank. One turn draws no shape worth reading, but
			an empty space below the prose reads as a chart that failed to load,
			and the reader has no way to tell that from one that is simply early.
		-->
		<span class="text-[0.6875rem] leading-relaxed text-muted-foreground">
			The shape of this window appears once a second turn has been measured.
		</span>
	{/if}
</div>
