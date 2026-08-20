import { fileURLToPath } from "node:url";

import { Checklist, strip_presentation_flags, type StepHandle } from "@artisanstreet/checklist";
import {
	NodeChildProcessSpawner,
	NodeFileSystem,
	NodePath,
	NodeRuntime,
} from "@effect/platform-node-shared";
import { Console, Data, Effect, Layer, Schema } from "effect";

export const ValidationArea = Schema.Literals([
	"frontend",
	"backend",
	"forge",
	"transport",
	"desktop",
	"native",
	"full",
]);
export type ValidationArea = typeof ValidationArea.Type;

export const validation_areas = {
	frontend: ["format:check:frontend", "lint:frontend", "check:frontend", "test:frontend"],
	backend: ["format:check:backend", "lint:backend", "check", "test:backend"],
	forge: ["format:check:forge", "lint:forge", "check:forge", "test:forge"],
	transport: ["format:check:transport", "lint:transport", "check", "test:transport"],
	desktop: ["format:check:desktop", "lint:desktop", "check:desktop", "test:desktop"],
	native: ["check:native", "test:native"],
	full: [
		"format:check",
		"lint",
		"check",
		"check:frontend",
		"check:forge",
		"test",
		"check:native",
		"test:native",
	],
} as const satisfies Record<ValidationArea, ReadonlyArray<string>>;

const validation_area_message = (area: string) =>
	`unknown validation area "${area}"; choose one of ${Object.keys(validation_areas).join(", ")}`;

export const select_validation_area = (area?: string): ReadonlyArray<string> => {
	const selected_area = area ?? "full";

	if (!Schema.is(ValidationArea)(selected_area)) {
		throw new Error(validation_area_message(selected_area));
	}

	return validation_areas[selected_area];
};

export const select_validation_scripts = (
	areas: ReadonlyArray<ValidationArea>,
): ReadonlyArray<string> => [...new Set(areas.flatMap((area) => validation_areas[area]))];

export class ValidationAreaError extends Data.TaggedError("ValidationAreaError")<{
	readonly area: string;
	readonly message: string;
}> {}

/** Surfaces the counts the underlying tools already print, on the step's own row. */
export const watch_validation_progress = (line: string, step: StepHandle): void => {
	const tests = /Tests\s+(\d+) passed\s*\((\d+)\)/u.exec(line);

	if (tests !== null) {
		step.progress(Number(tests[1]), Number(tests[2]));
		return;
	}

	const files = /Test Files\s+(\d+) passed\s*\((\d+)\)/u.exec(line);

	if (files !== null) {
		step.detail(`${files[1]}/${files[2]} files`);
		return;
	}

	const modules = /(\d+) modules transformed/u.exec(line);

	if (modules !== null) step.detail(`${modules[1]} modules`);
};

const PrintUsage = Effect.gen(function* () {
	yield* Console.log("Usage: pnpm run validate [-- <area> ...] [--no-tui|--json]");
	yield* Console.log(`Areas: ${Object.keys(validation_areas).join(", ")}`);
	yield* Console.log(
		"Run one Vitest target: pnpm run test:focus -- .tests/<area>/<file>.test.ts",
	);
});

export const ValidationProgram = Effect.gen(function* () {
	const requested_areas = strip_presentation_flags(process.argv.slice(2));

	if (requested_areas.some((area) => area === "--help" || area === "-h")) {
		yield* PrintUsage;
		return;
	}

	const selected_areas = yield* Effect.forEach(
		requested_areas.length === 0 ? ["full"] : requested_areas,
		(area) =>
			Schema.decodeUnknownEffect(ValidationArea)(area).pipe(
				Effect.mapError(
					() => new ValidationAreaError({ area, message: validation_area_message(area) }),
				),
			),
	);

	/**
	 * The gates stay sequential and deduplicated across areas, exactly as before:
	 * overlapping type-checks and native builds would contend for the same cores
	 * and interleave their output. Only the presentation changed.
	 */
	yield* Checklist.make(
		select_validation_scripts(selected_areas).map((script) => ({
			name: script,
			run: `pnpm run ${script}`,
			watch: watch_validation_progress,
		})),
		{ subtitle: selected_areas.join("+"), title: "validate" },
	);
});

const NodeProcessLive = NodeChildProcessSpawner.layer.pipe(
	Layer.provide(Layer.mergeAll(NodeFileSystem.layer, NodePath.layer)),
);

if (process.argv[1] === fileURLToPath(import.meta.url)) {
	NodeRuntime.runMain(ValidationProgram.pipe(Effect.provide(NodeProcessLive)));
}
