/** The number and unit a reset instant reads as, split so the number alone can tween. */
export interface ResetParts {
	readonly amount: number;
	readonly past: boolean;
	readonly unit: string;
}

/** Renders a reset instant as the largest sensible unit (`7d`, `3h`, `20m`). */
export const ResetPartsFor = (iso: string): ResetParts => {
	const diff_ms = new Date(iso).getTime() - Date.now();
	const abs_minutes = Math.round(Math.abs(diff_ms) / 60_000);
	const past = diff_ms < 0;

	if (abs_minutes < 60) return { amount: Math.max(1, abs_minutes), past, unit: "m" };
	if (abs_minutes < 60 * 24) return { amount: Math.round(abs_minutes / 60), past, unit: "h" };
	return { amount: Math.round(abs_minutes / (60 * 24)), past, unit: "d" };
};

/**
 * A quip, not a countdown: the first reading starts just short of its value and
 * settles onto it, so the number is legible the whole way rather than spinning
 * up from zero.
 */
export const RunUpFrom = (target: number): number =>
	Math.max(0, target - Math.max(1, Math.round(target * 0.08)));

/**
 * Solves a CSS `cubic-bezier(x1, y1, x2, y2)` for `y` at a given `x`, so a JS
 * tween can run the exact curve a CSS transition would rather than a stock
 * easing that merely resembles it. Newton-Raphson with a bisection fallback for
 * the flat stretches Newton converges poorly on.
 */
const CubicBezier = (x1: number, y1: number, x2: number, y2: number) => {
	const CurveA = (a: number, b: number) => 1 - 3 * b + 3 * a;
	const CurveB = (a: number, b: number) => 3 * b - 6 * a;
	const CurveC = (a: number) => 3 * a;

	const Sample = (t: number, a: number, b: number) =>
		((CurveA(a, b) * t + CurveB(a, b)) * t + CurveC(a)) * t;
	const Slope = (t: number, a: number, b: number) =>
		3 * CurveA(a, b) * t * t + 2 * CurveB(a, b) * t + CurveC(a);

	const SolveT = (x: number) => {
		let guess = x;

		for (let iteration = 0; iteration < 8; iteration += 1) {
			const slope = Slope(guess, x1, x2);
			if (slope === 0) break;
			guess -= (Sample(guess, x1, x2) - x) / slope;
		}

		if (guess >= 0 && guess <= 1) return guess;

		let low = 0;
		let high = 1;
		let mid = x;

		while (high - low > 1e-5) {
			mid = (low + high) / 2;
			if (Sample(mid, x1, x2) < x) low = mid;
			else high = mid;
		}

		return mid;
	};

	return (x: number) => (x <= 0 || x >= 1 ? x : Sample(SolveT(x), y1, y2));
};

const CUBIC_BEZIER_ARGUMENTS = /cubic-bezier\(([^)]+)\)/;

/** `--ease-smooth-out` — the token whose documented usage is "position change". */
export const MotionEasing = () =>
	Effect.gen(function* () {
		const fallback = CubicBezier(0.22, 1, 0.36, 1);
		const token = yield* RunBrowserDom(() => {
			if (typeof window === "undefined") return undefined;
			return getComputedStyle(document.documentElement)
				.getPropertyValue("--ease-smooth-out")
				.trim();
		});
		if (token === undefined) return fallback;

		const matched = CUBIC_BEZIER_ARGUMENTS.exec(token);
		if (matched === null) return fallback;

		const source = matched[1];
		if (source === undefined) return fallback;
		const points = source.split(",").map((part) => Number.parseFloat(part.trim()));
		if (points.length !== 4 || points.some((point) => Number.isNaN(point))) return fallback;
		const [x1, y1, x2, y2] = points;
		if (x1 === undefined || y1 === undefined || x2 === undefined || y2 === undefined) {
			return fallback;
		}
		return CubicBezier(x1, y1, x2, y2);
	});

/** Reads `--duration-fast` so the tween stays in step with the CSS motion scale. */
export const MotionDuration = () =>
	Effect.gen(function* () {
		const token = yield* RunBrowserDom(() => {
			if (typeof window === "undefined") return undefined;
			if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return "";
			return getComputedStyle(document.documentElement)
				.getPropertyValue("--duration-fast")
				.trim();
		});
		if (token === undefined || token === "") return 0;

		const parsed = Number.parseFloat(token);
		if (Number.isNaN(parsed)) return 250;
		return token.endsWith("ms") ? parsed : parsed * 1000;
	});
import { Effect } from "effect";
import { RunBrowserDom } from "$lib/browser/dom";
