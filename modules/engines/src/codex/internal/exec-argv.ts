import { Effect } from "effect";

import { type EngineOpenInput, EngineUnsupportedOperationError } from "../../engine";
import type { CodexProcessSpawnInput } from "../process";
import { MakeCodexExecPermissionArgs } from "./permissions";

/** Builds the validated argv-only process input for one Codex exec run. */
export function MakeCodexExecSpawn(
	input: EngineOpenInput,
	executable: string,
	executable_args: ReadonlyArray<string>,
	image_paths: ReadonlyArray<string> = [],
) {
	return Effect.gen(function* () {
		if (input._tag === "resume") {
			return yield* Effect.fail(
				new EngineUnsupportedOperationError({ engine_id: "codex", operation: "resume" }),
			);
		}

		const permission_args = yield* MakeCodexExecPermissionArgs(input);
		const args = [
			...executable_args,
			"exec",
			"--json",
			"--color",
			"never",
			"--cd",
			input.working_directory,
			...(input.model === undefined ? [] : ["--model", input.model]),
			...image_paths.flatMap((path) => ["--image", path]),
			...permission_args,
			"-",
		];

		return {
			args,
			command: executable,
		} satisfies CodexProcessSpawnInput;
	});
}
