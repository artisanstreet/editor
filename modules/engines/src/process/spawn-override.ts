import { Effect } from "effect";

import { EngineProcessError } from "../engine";
import type { EngineProcessFactory } from "./process";

/**
 * Redirects an engine adapter's spawns to a managed binary and config home.
 * `environment` is merged over the adapter's ordinary spawn environment, so a
 * `CLAUDE_CONFIG_DIR`/`CODEX_HOME` entry here always wins.
 */
export interface EngineSpawnOverride {
	readonly environment: Readonly<Record<string, string>>;
	readonly executable: string;
}

/**
 * Resolved before every spawn rather than at layer construction, so an
 * install, update, or rollback takes effect without rebuilding the engine
 * layer or restarting the Forge. This production resolver is deliberately
 * total: management composition must either return an owned executable or
 * fail as the same `EngineProcessError` an underlying spawn can report.
 *
 * Taking the profile per call rather than per layer is what allows two
 * accounts of one engine to run concurrently: each spawn resolves its own
 * home while sharing the single installed binary.
 */
export type ResolveEngineSpawnOverride = (
	profile_id?: string,
) => Effect.Effect<EngineSpawnOverride, EngineProcessError>;

/**
 * Wraps a process factory so every spawn consults the override first. All of
 * an adapter's process starts — probes, runs, usage reads — flow through its
 * factory, which makes this one seam sufficient to repoint an entire engine.
 * A spawn that names no profile resolves the engine's default one, so an
 * adapter that has no profile to pass keeps its prior behaviour exactly.
 */
export const with_engine_spawn_override = (
	factory: typeof EngineProcessFactory.Service,
	ResolveOverride: ResolveEngineSpawnOverride,
): typeof EngineProcessFactory.Service => ({
	Spawn: (input) =>
		Effect.gen(function* () {
			const override = yield* ResolveOverride(input.profile_id);
			return yield* factory.Spawn({
				...input,
				command: override.executable,
				env: { ...(input.env ?? process.env), ...override.environment },
			});
		}),
});
