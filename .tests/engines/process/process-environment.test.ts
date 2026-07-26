import { Buffer } from "node:buffer";

import * as NodeFileSystem from "@effect/platform-node-shared/NodeFileSystem";
import { describe, expect, it } from "vitest";
import { Effect, Layer } from "effect";

import {
	EngineProcessEnvironment,
	EngineProcessFactory,
	EngineProcessFactoryLayer,
	MakeEngineProcessEnvironmentLayer,
} from "@artisan/engines";

const RuntimeLayer = (environment: NodeJS.ProcessEnv = {}) =>
	MakeEngineProcessEnvironmentLayer({
		environment,
		exec_path: "fallback-node",
		is_electron: false,
		platform: process.platform,
	}).pipe(Layer.provide(NodeFileSystem.layer));

const ReadAll = (stream: AsyncIterable<Uint8Array>) =>
	Effect.tryPromise({
		try: async () => {
			const chunks: Array<Uint8Array> = [];

			for await (const chunk of stream) {
				chunks.push(chunk);
			}

			return Buffer.concat(chunks).toString("utf8");
		},
		catch: (cause) => cause,
	});

describe("Engine process environment", () => {
	it("prefers explicit executable and Windows host configuration", async () => {
		const service = await Effect.runPromise(
			EngineProcessEnvironment.pipe(
				Effect.provide(
					RuntimeLayer({
						ARTISAN_NODE_EXECUTABLE: "configured-node",
						ARTISAN_WINDOWS_PROCESS_HOST: "C:\\Artisan\\host.js",
						EXPLICIT_MARKER: "preserved",
					}),
				),
			),
		);

		expect(service.node_executable).toBe("configured-node");
		expect(service.windows_process_host_path).toBe("C:\\Artisan\\host.js");
		expect(service.environment).toEqual({
			ARTISAN_NODE_EXECUTABLE: "configured-node",
			ARTISAN_WINDOWS_PROCESS_HOST: "C:\\Artisan\\host.js",
			EXPLICIT_MARKER: "preserved",
		});
	});

	it("falls back to the supplied runtime executable", async () => {
		const service = await Effect.runPromise(
			EngineProcessEnvironment.pipe(Effect.provide(RuntimeLayer())),
		);

		expect(service.node_executable).toBe("fallback-node");
	});

	it.skipIf(process.platform === "win32")(
		"passes only the explicitly injected child environment",
		async () => {
			const factory_layer = EngineProcessFactoryLayer.pipe(
				Layer.provide(RuntimeLayer({ HOST_ONLY_MARKER: "not-inherited" })),
			);
			const output = await Effect.runPromise(
				Effect.gen(function* () {
					const factory = yield* EngineProcessFactory;
					const handle = yield* factory.Spawn({
						args: [
							"-e",
							"process.stdout.write(`${process.env.EXPLICIT_MARKER}:${String(process.env.HOST_ONLY_MARKER)}`)",
						],
						command: process.execPath,
						env: { EXPLICIT_MARKER: "child" },
					});
					const stdout = yield* ReadAll(handle.Stdout);

					yield* handle.Exit;

					return stdout;
				}).pipe(Effect.scoped, Effect.provide(factory_layer)),
			);

			expect(output).toBe("child:undefined");
		},
	);
});
