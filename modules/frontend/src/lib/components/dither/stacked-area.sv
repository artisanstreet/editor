<script lang="ts" effect>
	import { Effect } from "effect";
	import type { Snippet } from "svelte";
	import { RunBrowserDom } from "$lib/browser/dom";
	import {
		BackingSize,
		dither_cell,
		PaintColumn,
		Resample,
		type DitherVariant,
	} from "./paint";

	/**
	 * A two-series stacked dithered area chart.
	 *
	 * Stacked rather than overlaid because the two series are parts of one whole:
	 * overlaying would let the smaller sit inside the larger and imply they are
	 * alternatives. The upper band's height is its own value; the outline is the
	 * total.
	 */
	let {
		height = 120,
		lower,
		lower_color,
		lower_variant = "gradient",
		upper,
		upper_color,
		tooltip,
		upper_variant = "hatched",
		width = 256,
	}: {
		height?: number;
		/** The baseline series, drawn from zero. */
		lower: ReadonlyArray<number>;
		lower_color: readonly [number, number, number];
		lower_variant?: DitherVariant;
		/** Stacked on top of `lower`; the two are read pairwise. */
		upper: ReadonlyArray<number>;
		upper_color: readonly [number, number, number];
		/**
		 * Rendered beside the hovered point. Supplied by the caller because the
		 * chart knows where a point is but nothing about what it means.
		 */
		tooltip?: Snippet<[{ index: number }]>;
		upper_variant?: DitherVariant;
		width?: number;
	} = $props();

	let canvas = $state<HTMLCanvasElement | null>(null);
	let hover_index = $state<number | undefined>(undefined);

	const count = $derived(Math.min(lower.length, upper.length));
	const totals = $derived(
		Array.from({ length: count }, (_, index) => (lower[index] ?? 0) + (upper[index] ?? 0)),
	);
	/**
	 * Scaled to the largest total rather than to the context window: a window
	 * that is 11% full would otherwise draw as a flat line hugging the axis,
	 * saying nothing about how it filled. The percentage is stated in words.
	 */
	const peak = $derived(Math.max(1, ...totals));

	const Paint = (
		element: HTMLCanvasElement | null,
		series_lower: ReadonlyArray<number>,
		series_totals: ReadonlyArray<number>,
		scale_peak: number,
	) =>
		Effect.gen(function* () {
			if (element === null || series_totals.length === 0) return;
			const { cols, rows } = BackingSize(width, height);

			yield* RunBrowserDom(() => {
				const context = element.getContext("2d");
				if (context === null) return;
				element.width = cols;
				element.height = rows;
				context.clearRect(0, 0, cols, rows);

				/** Colour belongs to the caller; the engine only decides alpha. */
				const fill_with =
					([red, green, blue]: readonly [number, number, number]) =>
					(x: number, y: number, alpha: number) => {
						context.fillStyle = `rgba(${red}, ${green}, ${blue}, ${alpha})`;
						context.fillRect(x, y, 1, 1);
					};
				const fill_upper = fill_with(upper_color);
				const fill_lower = fill_with(lower_color);

				/** One backing pixel per dither cell; the row is the value's own scale. */
				const row_of = (value: number) => rows - (value / scale_peak) * (rows - 1);
				const lower_rows = Resample(series_lower.map(row_of), cols);
				const total_rows = Resample(series_totals.map(row_of), cols);

				for (let x = 0; x < cols; x += 1) {
					const floor = lower_rows[x] ?? rows;
					const top = total_rows[x] ?? rows;
					/**
					 * The stacked band sits between the two curves, so the lower
					 * series' own line is its floor.
					 */
					PaintColumn(fill_upper, x, Math.min(top, floor), Math.max(top, floor), {
						stacked: true,
						variant: upper_variant,
					});
					PaintColumn(fill_lower, x, floor, rows, { variant: lower_variant });
				}
			});
		});

	yield* Paint(canvas, lower, totals, peak);

	const rgba = ([red, green, blue]: readonly [number, number, number], alpha: number) =>
		`rgba(${red}, ${green}, ${blue}, ${alpha})`;
	/** Lower first so the upper series' markers sit above where the two curves meet. */
	const series = $derived([
		{ halo: rgba(lower_color, 0.18), key: "lower", stroke: rgba(lower_color, 1), values: lower },
		{ halo: rgba(upper_color, 0.18), key: "upper", stroke: rgba(upper_color, 1), values: totals },
	]);

	/** Point geometry in CSS pixels, mirroring the backing canvas' own scale. */
	const x_of = (index: number) => (count === 1 ? width / 2 : (index / (count - 1)) * width);
	const y_of = (value: number) => height - (value / peak) * (height - dither_cell);

	const TrackPointer = (event: PointerEvent) =>
		Effect.gen(function* () {
			const target = event.currentTarget;
			if (!(target instanceof HTMLElement)) return;
			const offset = yield* RunBrowserDom(
				() => event.clientX - target.getBoundingClientRect().left,
			);
			/** Nearest point rather than the enclosing band: the dots are what is hoverable. */
			hover_index = Math.max(
				0,
				Math.min(count - 1, Math.round((offset / Math.max(1, width)) * (count - 1))),
			);
		});
	const ClearPointer = Effect.gen(function* () {
		hover_index = undefined;
	});
</script>

{#if count > 0}
	<div
		class="relative"
		style={`width: ${width}px; height: ${height}px`}
		role="presentation"
		onpointermove={yield* TrackPointer(event)}
		onpointerleave={yield* ClearPointer}
	>
		<!--
			Rendered low and scaled up: the backing canvas is one pixel per dither
			cell, and `pixelated` is what keeps the cells hard-edged instead of
			resampling them back into a smooth gradient.
		-->
		<canvas
			bind:this={canvas}
			aria-hidden="true"
			class="dither-canvas block"
			style={`width: ${width}px; height: ${height}px`}
		></canvas>

		<!--
			Markers live in SVG over the canvas rather than in it: a dot drawn into
			the backing canvas would be dithered and pixel-snapped along with
			everything else, and these are the one thing that has to stay crisp.
		-->
		<svg
			class="pointer-events-none absolute inset-0"
			{width}
			{height}
			viewBox={`0 0 ${width} ${height}`}
			aria-hidden="true"
		>
			{#each series as line (line.key)}
				{#each line.values as value, index (index)}
					<circle
						cx={x_of(index)}
						cy={y_of(value)}
						r="2"
						fill="var(--card, #0b0b0c)"
						stroke={line.stroke}
						stroke-width="1"
					/>
				{/each}
			{/each}

			{#if hover_index !== undefined}
				{@const active = hover_index}
				{#each series as line (line.key)}
					{@const value = line.values[active] ?? 0}
					<!-- A halo, so the hovered point is unmistakable over the texture. -->
					<circle cx={x_of(active)} cy={y_of(value)} r="5" fill={line.halo} />
					<circle
						cx={x_of(active)}
						cy={y_of(value)}
						r="3"
						fill="var(--card, #0b0b0c)"
						stroke={line.stroke}
						stroke-width="2"
					/>
				{/each}
			{/if}
		</svg>

		{#if hover_index !== undefined && tooltip !== undefined}
			{@const active = hover_index}
			<!--
				Anchored to the point and clamped inside the plot, so a reading near
				either edge stays legible instead of hanging off the card.
			-->
			<div
				class="pointer-events-none absolute z-10 -translate-x-1/2 -translate-y-full"
				style={`left: ${Math.min(width - 4, Math.max(4, x_of(active)))}px; top: ${Math.max(
					0,
					y_of(Math.max(lower[active] ?? 0, totals[active] ?? 0)) - 10,
				)}px`}
			>
				{@render tooltip({ index: active })}
			</div>
		{/if}
	</div>
{/if}

<style>
	.dither-canvas {
		image-rendering: pixelated;
	}
</style>
