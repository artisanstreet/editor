import type { SurfaceUsageAggregate } from "@artisan/protocol";

const exact_tokens = new Intl.NumberFormat("en");

/**
 * States the whole context reading as one sentence, for the control that owns
 * the gauge to describe itself with.
 *
 * The gauge is a mark rather than a control, so it cannot carry this itself;
 * the visual tooltip only exists while it is open, which leaves nothing for a
 * screen reader on a focused-but-unopened trigger. Written once here so the
 * spoken reading and the rendered one cannot drift apart.
 */
export const ContextUsageDescription = (
	usage: SurfaceUsageAggregate,
	window_tokens: number,
): string => {
	const breakdown = [
		{ label: "Input", value: usage.input_tokens },
		{ label: "Cached input", value: usage.cached_input_tokens },
		{ label: "Output", value: usage.output_tokens },
	].filter((row): row is { label: string; value: number } => row.value !== undefined);

	const totals = `Context window contains ${exact_tokens.format(
		usage.context_tokens ?? 0,
	)} of ${exact_tokens.format(window_tokens)} tokens.`;

	return breakdown.length === 0
		? `${totals} No detailed token breakdown is available.`
		: `${totals} ${breakdown
				.map((row) => `${row.label}: ${exact_tokens.format(row.value)} tokens.`)
				.join(" ")}`;
};
