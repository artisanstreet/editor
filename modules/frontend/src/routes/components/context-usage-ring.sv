<script lang="ts">
	import {
		Tooltip,
		TooltipContent,
		TooltipProvider,
		TooltipTrigger,
	} from "$lib/components/ui/tooltip";

	/**
	 * Presentational context-window gauge for the composer. The percent is
	 * derived by the caller because the window denominator may come from the
	 * provider wire or from the catalog fallback.
	 */
	let {
		cached_input_tokens,
		context_tokens,
		input_tokens,
		output_tokens,
		percent,
		window_tokens,
	}: {
		readonly cached_input_tokens?: number;
		readonly context_tokens: number;
		readonly input_tokens?: number;
		readonly output_tokens?: number;
		readonly percent: number;
		readonly window_tokens: number;
	} = $props();

	const ring_radius = 6.5;
	const ring_circumference = 2 * Math.PI * ring_radius;
	/** Providers begin auto-compacting around 85–90%; warn in the same territory. */
	const nearly_full = $derived(percent >= 85);

	const compact_tokens = new Intl.NumberFormat("en", {
		maximumFractionDigits: 1,
		notation: "compact",
	});
	const exact_tokens = new Intl.NumberFormat("en");
	const breakdown = $derived(
		[
			{ label: "Input", value: input_tokens },
			{ label: "Cached input", value: cached_input_tokens },
			{ label: "Output", value: output_tokens },
		].filter(
			(row): row is { label: string; value: number } => row.value !== undefined,
		),
	);
	const accessible_breakdown = $derived(
		breakdown.length === 0
			? "No detailed token breakdown is available."
			: breakdown.map((row) => `${row.label}: ${exact_tokens.format(row.value)} tokens.`).join(" "),
	);
</script>

<TooltipProvider delayDuration={0}>
	<Tooltip>
		<TooltipTrigger>
			{#snippet child({ props: tooltip_props })}
				<button
					type="button"
					{...tooltip_props}
					class={nearly_full
						? "flex cursor-default items-center gap-1.5 text-destructive focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none"
						: "flex cursor-default items-center gap-1.5 text-muted-foreground transition-colors duration-(--duration-fast) hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none motion-reduce:transition-none"}
					aria-label={`Context window ${Math.round(percent)}% full`}
					aria-describedby="context-usage-details"
				>
					<svg viewBox="0 0 16 16" class="size-4 -rotate-90" aria-hidden="true">
						<circle
							cx="8"
							cy="8"
							r={ring_radius}
							fill="none"
							stroke="var(--muted)"
							stroke-width="2.5"
						/>
						<circle
							cx="8"
							cy="8"
							r={ring_radius}
							fill="none"
							stroke="currentColor"
							stroke-width="2.5"
							stroke-linecap="round"
							class="transition-[stroke-dasharray] duration-(--duration-quick) ease-(--ease-smooth-out) motion-reduce:transition-none"
							stroke-dasharray={`${(percent / 100) * ring_circumference} ${ring_circumference}`}
						/>
					</svg>
					<span class="text-xs tabular-nums">{Math.round(percent)}%</span>
				</button>
			{/snippet}
		</TooltipTrigger>
		<span id="context-usage-details" class="sr-only">
			Context window contains {exact_tokens.format(context_tokens)} of {exact_tokens.format(window_tokens)} tokens. {accessible_breakdown}
		</span>
		<TooltipContent side="top" class="flex-col items-start gap-1.5">
			<span class="font-medium">
				Context window <span class="tabular-nums">{Math.round(percent)}%</span> full
			</span>
			<span class="text-background/70">
				{compact_tokens.format(context_tokens)} of {compact_tokens.format(window_tokens)} tokens
			</span>
			{#if breakdown.length > 0}
				<div class="flex w-full flex-col gap-0.5 border-t border-background/20 pt-1.5">
					{#each breakdown as row (row.label)}
						<span class="flex w-full items-center justify-between gap-4">
							<span class="text-background/70">{row.label}</span>
							<span class="tabular-nums">{exact_tokens.format(row.value)}</span>
						</span>
					{/each}
				</div>
			{/if}
		</TooltipContent>
	</Tooltip>
</TooltipProvider>
