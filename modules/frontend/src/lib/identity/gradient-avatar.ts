/**
 * Seeded, dithered gradient avatars, in place of initials.
 *
 * A machine identifies itself with a stable string (its hostname), so the same
 * host always draws the same avatar without storing anything: the seed picks
 * one Tailwind hue, and the tile ramps that hue's 600 into its 400 toward the
 * top-right corner.
 *
 * The ramp is an ordered dither rather than a smooth gradient, using the same
 * Bayer 4×4 matrix and density clamp as vsx's `dither-gradient` component. Two
 * flat shades and a threshold replace that component's per-pixel colour mix —
 * the palette values stay exactly Tailwind's, and the tile survives being
 * scaled or serialised without banding.
 */

/**
 * Tailwind's chromatic families only — the neutrals (slate, gray, zinc,
 * neutral, stone) and the theme's own greys read as a disabled avatar rather
 * than an identity. Values are copied from `tailwindcss/theme.css` rather than
 * referenced as `var(--color-red-600)`, so the markup renders identically when
 * it is inlined, serialised into a data URL, or drawn outside the app's theme.
 */
const avatar_palette = [
	{ name: "red", from: "oklch(57.7% 0.245 27.325)", to: "oklch(70.4% 0.191 22.216)" },
	{ name: "orange", from: "oklch(64.6% 0.222 41.116)", to: "oklch(75% 0.183 55.934)" },
	{ name: "amber", from: "oklch(66.6% 0.179 58.318)", to: "oklch(82.8% 0.189 84.429)" },
	{ name: "yellow", from: "oklch(68.1% 0.162 75.834)", to: "oklch(85.2% 0.199 91.936)" },
	{ name: "lime", from: "oklch(64.8% 0.2 131.684)", to: "oklch(84.1% 0.238 128.85)" },
	{ name: "green", from: "oklch(62.7% 0.194 149.214)", to: "oklch(79.2% 0.209 151.711)" },
	{ name: "emerald", from: "oklch(59.6% 0.145 163.225)", to: "oklch(76.5% 0.177 163.223)" },
	{ name: "teal", from: "oklch(60% 0.118 184.704)", to: "oklch(77.7% 0.152 181.912)" },
	{ name: "cyan", from: "oklch(60.9% 0.126 221.723)", to: "oklch(78.9% 0.154 211.53)" },
	{ name: "sky", from: "oklch(58.8% 0.158 241.966)", to: "oklch(74.6% 0.16 232.661)" },
	{ name: "blue", from: "oklch(54.6% 0.245 262.881)", to: "oklch(70.7% 0.165 254.624)" },
	{ name: "indigo", from: "oklch(51.1% 0.262 276.966)", to: "oklch(67.3% 0.182 276.935)" },
	{ name: "violet", from: "oklch(54.1% 0.281 293.009)", to: "oklch(70.2% 0.183 293.541)" },
	{ name: "purple", from: "oklch(55.8% 0.288 302.321)", to: "oklch(71.4% 0.203 305.504)" },
	{ name: "fuchsia", from: "oklch(59.1% 0.293 322.896)", to: "oklch(74% 0.238 322.16)" },
	{ name: "pink", from: "oklch(59.2% 0.249 0.584)", to: "oklch(71.8% 0.202 349.761)" },
	{ name: "rose", from: "oklch(58.6% 0.253 17.585)", to: "oklch(71.2% 0.194 13.428)" },
] as const;

export type GradientAvatarColor = (typeof avatar_palette)[number];

/** vsx's threshold matrix, normalised so each cell sits mid-step. */
const Bayer4 = [
	[0, 8, 2, 10],
	[12, 4, 14, 6],
	[3, 11, 1, 9],
	[15, 7, 13, 5],
].map((row) => row.map((value) => (value + 0.5) / 16));

/** Cells across the tile; 16 lands on 2px cells at the sidebar's 32px avatar. */
const avatar_cells = 16;

/**
 * vsx caps density short of 1 so the lit end keeps its grain instead of
 * flattening into a solid block. The same cap keeps a few 600 cells alive in
 * the avatar's top-right corner.
 */
const maximum_density = 0.9;

const clamp = (value: number) => Math.min(1, Math.max(0, value));

/**
 * FNV-1a. The avatar only needs a stable, well-spread bucket per seed, and a
 * hash that runs identically in every runtime keeps a host's colour from
 * shifting between the desktop shell and a browser Forge.
 */
const SeedHash = (seed: string): number => {
	let hash = 0x811c9dc5;
	for (let index = 0; index < seed.length; index += 1) {
		hash ^= seed.charCodeAt(index);
		hash = Math.imul(hash, 0x01000193) >>> 0;
	}
	return hash;
};

/** The palette entry a seed resolves to, for callers that want the colour itself. */
export const GradientAvatarColorFor = (seed: string): GradientAvatarColor =>
	avatar_palette[SeedHash(seed) % avatar_palette.length] ?? avatar_palette[0];

/** Only the characters that could close the attribute or the tag around it. */
const EscapeAttribute = (value: string): string =>
	value.replaceAll("&", "&amp;").replaceAll('"', "&quot;").replaceAll("<", "&lt;");

/**
 * Horizontal runs of lit cells, one row at a time. A rect per cell would be
 * 256 nodes for a 32px avatar; merging runs typically leaves a couple of dozen.
 */
const LitRunsFor = (): ReadonlyArray<{ x: number; y: number; width: number }> => {
	const runs: Array<{ x: number; y: number; width: number }> = [];

	for (let y = 0; y < avatar_cells; y += 1) {
		let run_start: number | undefined = undefined;

		for (let x = 0; x <= avatar_cells; x += 1) {
			/** Progress along the bottom-left → top-right diagonal. */
			const horizontal_progress = (x + 0.5) / avatar_cells;
			const vertical_progress = 1 - (y + 0.5) / avatar_cells;
			const density = Math.min(
				maximum_density,
				clamp((horizontal_progress + vertical_progress) / 2),
			);
			const lit = x < avatar_cells && density > (Bayer4[y & 3]?.[x & 3] ?? 1);

			if (lit && run_start === undefined) run_start = x;
			if (!lit && run_start !== undefined) {
				runs.push({ width: x - run_start, x: run_start, y });
				run_start = undefined;
			}
		}
	}

	return runs;
};

/** The cell grid never varies, so the run layout is computed once per module. */
const avatar_runs = LitRunsFor();

/**
 * Renders the avatar as standalone SVG markup. The dither runs from the hue's
 * 600 at the bottom-left to its 400 at the top-right, always on that diagonal,
 * so a row of avatars shares one light direction. The tile fills its box and
 * stays square; rounding is the caller's (the Avatar root clips it).
 */
export const GradientAvatarSvg = (seed: string, title?: string): string => {
	const color = GradientAvatarColorFor(seed);
	const label =
		title === undefined
			? ' aria-hidden="true"'
			: ` role="img" aria-label="${EscapeAttribute(title)}"`;
	const lit = avatar_runs
		.map((run) => `<rect x="${run.x}" y="${run.y}" width="${run.width}" height="1"/>`)
		.join("");

	return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${avatar_cells} ${avatar_cells}" width="100%" height="100%" preserveAspectRatio="xMidYMid slice" shape-rendering="crispEdges"${label}><rect width="${avatar_cells}" height="${avatar_cells}" fill="${color.from}"/><g fill="${color.to}">${lit}</g></svg>`;
};
