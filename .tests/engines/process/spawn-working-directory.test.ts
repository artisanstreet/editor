import { tmpdir } from "node:os";
import { join } from "node:path";

import * as NodeFileSystem from "@effect/platform-node-shared/NodeFileSystem";
import { describe, expect, it } from "vitest";
import { Effect, Layer } from "effect";

import {
	EngineProcessError,
	EngineProcessFactory,
	EngineProcessFactoryLayer,
	MakeEngineProcessEnvironmentLayer,
	engine_process_working_directory_missing_code,
} from "@artisan/engines";

const FactoryLayer = EngineProcessFactoryLayer.pipe(
	Layer.provide(
		MakeEngineProcessEnvironmentLayer({
			environment: {},
			exec_path: process.execPath,
			is_electron: false,
			is_sea: false,
			platform: process.platform,
		}).pipe(Layer.provide(NodeFileSystem.layer)),
	),
);

describe("engine process spawn working-directory guard", () => {
	/**
	 * The guard fails before any platform spawn machinery runs, so this test
	 * never launches a process: a missing folder must surface as its own
	 * classified failure rather than an opaque instant process death that
	 * settles into the transcript as a generic startup failure.
	 */
	it("refuses a spawn whose working directory is gone, as a classified failure", async () => {
		const missing = join(tmpdir(), `artisan-missing-working-directory-${process.pid}`);
		const failure = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const factory = yield* EngineProcessFactory;

					return yield* factory
						.Spawn({
							args: ["--version"],
							command: process.execPath,
							cwd: missing,
						})
						.pipe(Effect.flip);
				}),
			).pipe(Effect.provide(FactoryLayer)),
		);

		expect(failure).toBeInstanceOf(EngineProcessError);
		expect(failure.operation).toBe("spawn");
		expect(failure.artisan_code).toBe(engine_process_working_directory_missing_code);
		expect(failure.message).toContain("Working directory does not exist");
	});

	/** A spawn with no cwd keeps its existing behaviour: the guard only reads a supplied one. */
	it("does not consult the filesystem when no working directory is supplied", async () => {
		const spawned = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const factory = yield* EngineProcessFactory;
					const handle = yield* factory.Spawn({
						args: ["--version"],
						command: process.execPath,
					});
					const exit = yield* handle.Exit;
					yield* handle.Close;

					return exit;
				}),
			).pipe(Effect.provide(FactoryLayer)),
		);

		expect(spawned.code).toBe(0);
	});
});
