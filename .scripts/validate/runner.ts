import { fileURLToPath } from "node:url";

import {
	NodeChildProcessSpawner,
	NodeFileSystem,
	NodePath,
	NodeRuntime,
} from "@effect/platform-node-shared";
import { Console, Data, Effect, Layer, Schema } from "effect";
import { ChildProcess } from "effect/unstable/process";

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

export class ValidationScriptError extends Data.TaggedError("ValidationScriptError")<{
	readonly exit_code: number;
	readonly message: string;
	readonly script: string;
}> {}

const RunScript = (script: string) =>
	Effect.gen(function* () {
		// Windows package-manager shims are command scripts. Keep the selected
		// package script in one fixed shell command so Node does not concatenate
		// a separately supplied argv (DEP0190); every value comes from the map above.
		const command = process.platform === "win32" ? `pnpm run ${script}` : "pnpm";
		const arguments_ = process.platform === "win32" ? [] : ["run", script];
		const child = yield* ChildProcess.make(command, arguments_, {
			shell: process.platform === "win32",
			stderr: "inherit",
			stdin: "inherit",
			stdout: "inherit",
		});
		const exit_code = yield* child.exitCode;
		if (exit_code !== 0) {
			return yield* Effect.fail(
				new ValidationScriptError({
					exit_code,
					message: `pnpm run ${script} failed with exit code ${exit_code}`,
					script,
				}),
			);
		}
	});

const PrintUsage = Effect.gen(function* () {
	yield* Console.log("Usage: pnpm run validate [-- <area> ...]");
	yield* Console.log(`Areas: ${Object.keys(validation_areas).join(", ")}`);
	yield* Console.log(
		"Run one Vitest target: pnpm run test:focus -- .tests/<area>/<file>.test.ts",
	);
});

export const ValidationProgram = Effect.gen(function* () {
	const requested_areas = process.argv.slice(2);

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
	const scripts = select_validation_scripts(selected_areas);

	yield* Console.log(`[validate] ${selected_areas.join("+")}: ${scripts.join(" -> ")}`);
	for (const script of scripts) {
		yield* RunScript(script);
	}
}).pipe(Effect.scoped);

const NodeProcessLive = NodeChildProcessSpawner.layer.pipe(
	Layer.provide(Layer.mergeAll(NodeFileSystem.layer, NodePath.layer)),
);

if (process.argv[1] === fileURLToPath(import.meta.url)) {
	NodeRuntime.runMain(ValidationProgram.pipe(Effect.provide(NodeProcessLive)));
}
