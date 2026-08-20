import { readdirSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";

/**
 * Anchored to this file, not to the working directory: a sibling suite boots a
 * Vite server and `chdir`s into the frontend for the length of its run, and the
 * Windows config shares one worker, so a cwd-relative path resolves differently
 * depending on which files ran first.
 */
const StylesDirectory = resolve(import.meta.dirname, "../../modules/frontend/src/lib/styles");

/**
 * Every core stylesheet as one string.
 *
 * The core is split by what a rule fundamentally is — tokens, utilities,
 * keyframes, authored-elsewhere markup, unowned markup — not by which feature
 * it serves. So a test asserting that a rule exists should not also assert
 * which of the five files it landed in: that couples the suite to a filing
 * decision rather than to behaviour, and every reshuffle then breaks tests that
 * are still true.
 */
export const ReadStylesheets = () =>
	readdirSync(StylesDirectory)
		.filter((entry) => entry.endsWith(".css"))
		.sort()
		.map((entry) => readFileSync(join(StylesDirectory, entry), "utf8"))
		.join("\n");
