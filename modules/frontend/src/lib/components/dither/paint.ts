/**
 * Ordered-dither painting primitives, ported from dither-kit's canvas engine.
 *
 * The texture is painted onto a deliberately low-resolution backing canvas —
 * one backing pixel per dither cell — which is then scaled up with
 * `image-rendering: pixelated`. Drawing the dots at display resolution instead
 * would need one node or one fill per cell and would lose the hard pixel edges
 * that make it read as a dither rather than as a gradient.
 */

/** The 4×4 ordered (Bayer) matrix, normalised to 0–1 thresholds. */
export const bayer_thresholds: ReadonlyArray<ReadonlyArray<number>> = [
	[0, 8, 2, 10],
	[12, 4, 14, 6],
	[3, 11, 1, 9],
	[15, 7, 13, 5],
].map((row) => row.map((value) => (value + 0.5) / 16));

/** CSS pixels per dither cell. Chunky enough to read as deliberate pixel art. */
export const dither_cell = 2;
const max_cols = 520;
const max_rows = 200;

/**
 * Opacity of the top outline. Just under solid, so the edge reads as the shape
 * ending rather than as a line drawn over it.
 */
const border_alpha = 0.72;

/**
 * Opacity of an unlit cell relative to a lit one.
 *
 * This is the rule the whole engine turns on: the scatter modulates between two
 * tiers of the *same* colour rather than leaving holes. A hole shows whatever
 * sits behind the chart, which reads as a stark speck on a light surface and
 * vanishes on a dark one; two alpha tiers of one colour behave on both.
 */
const off_tier = 0.4;

/** How a series' fill is textured. */
export type DitherVariant = "gradient" | "hatched" | "dotted" | "solid";

/**
 * Paints one backing-canvas cell at the given alpha.
 *
 * The engine takes a callback rather than a canvas context so it owns no DOM
 * type at all: the whole texture is decided by arithmetic, and keeping the
 * surface out of it means the ordering and density can be asserted by value.
 */
export type FillCell = (x: number, y: number, alpha: number) => void;

export interface PaintColumnOptions {
	/** Thins the texture so overlapping layers stay legible as separate layers. */
	readonly sparse?: number;
	/** Denser, with a solid floor — for a series stacked on another. */
	readonly stacked?: boolean;
	readonly variant?: DitherVariant;
}

export const clamp01 = (value: number) => (value < 0 ? 0 : value > 1 ? 1 : value);

/**
 * Fills one backing-canvas column from `top` down to `floor`: solid at the
 * floor, dissolving upward so the fill fades out toward its own value line,
 * then caps the top with a soft outline.
 */
export const PaintColumn = (
	fill: FillCell,
	x: number,
	top: number,
	floor: number,
	{ sparse = 0, stacked = false, variant = "gradient" }: PaintColumnOptions,
) => {
	const from = Math.round(top);
	const to = Math.round(floor);
	const depth = to - from;

	if (depth <= 0) {
		fill(x, from, border_alpha);
		return;
	}

	const bias = (variant === "dotted" ? 0.12 : 0) + (stacked ? 0.2 : 0) - sparse;

	for (let y = from; y < to; y += 1) {
		/** Inverted falloff: 0 at the value line, 1 at the floor. */
		const raw = (y - from) / depth;
		const density = stacked ? 0.5 + 0.5 * raw : raw;
		/** A diagonal comb, which is what gives the hatched variant its weave. */
		if (variant === "hatched" && ((x + y) & 3) >= 2) continue;
		const row = bayer_thresholds[y & 3] ?? [];
		const threshold = row[x & 3] ?? 0;
		const lit = variant === "solid" || density > threshold - bias;
		/** Only `dotted` keeps real gaps; every other variant rides the alpha. */
		if (variant === "dotted" && !lit) continue;
		const tier = 0.3 + density * 0.7;

		fill(x, y, clamp01(lit ? tier : tier * off_tier));
	}

	/**
	 * The outline, plus a fainter feather beneath it, so the edge reads as soft
	 * rather than as a hard rule floating above the fade.
	 */
	fill(x, from, border_alpha);
	if (depth > 1) fill(x, from + 1, border_alpha * 0.5);
};

/**
 * Linear-resamples a per-index series to one value per column.
 *
 * Linear rather than a spline on purpose: every drawn value then lies between
 * two the provider actually reported. A spline would invent intermediate peaks,
 * which for a token series means drawing a window fuller than it ever was.
 */
export const Resample = (source: ReadonlyArray<number>, cols: number): ReadonlyArray<number> => {
	const last = Math.max(source.length - 1, 1);

	return Array.from({ length: cols }, (_, column) => {
		const position = (column / Math.max(cols - 1, 1)) * last;
		/**
		 * Clamped, unlike the reference implementation: with a single sample the
		 * index runs one past the end on the final column, and an unclamped read
		 * falls back to zero — drawing a series that dives to the axis at its
		 * right edge purely as an artefact of having one point.
		 */
		const index = Math.min(Math.floor(position), source.length - 1);
		const fraction = position - index;
		const start = source[index] ?? 0;
		const end = source[Math.min(index + 1, source.length - 1)] ?? start;

		return start + (end - start) * fraction;
	});
};

/** Backing-canvas resolution for a plot rect — low-res, scaled up pixelated. */
export const BackingSize = (width: number, height: number) => ({
	cols: Math.min(max_cols, Math.max(8, Math.round(width / dither_cell))),
	rows: Math.min(max_rows, Math.max(8, Math.round(height / dither_cell))),
});
