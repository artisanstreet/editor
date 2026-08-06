<script lang="ts">
	import { Progress } from "$lib/components/ui/progress";

	/**
	 * The reading behind the gauge, as a card body.
	 *
	 * Deliberately not a by-category breakdown. Neither harness discloses one —
	 * both report totals only — and Artisan never holds the assembled prompt, so
	 * any "system prompt vs skills vs messages" split here would be invented.
	 * The bar below restates the same single fact the prose carries: how full
	 * the window is now.
	 */
	let {
		model_name,
		percent,
		window_tokens,
	}: {
		readonly model_name?: string;
		readonly percent: number;
		readonly window_tokens: number;
	} = $props();

	/**
	 * Whole units only. A window size is a round capacity a reader recognises —
	 * "258K" is the fact; the ".4" is noise that reads as precision the number
	 * does not carry.
	 */
	const compact_tokens = new Intl.NumberFormat("en", {
		maximumFractionDigits: 0,
		notation: "compact",
	});

	const model = $derived(model_name ?? "this model");
	const fill = $derived(Math.min(100, Math.max(0, percent)));
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

	<Progress value={fill} max={100} aria-label={`Context window ${Math.round(percent)}% full`} />
</div>
