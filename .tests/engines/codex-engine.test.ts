import { fileURLToPath } from "node:url";

import { afterEach, describe, expect, it } from "vitest";
import { Effect, Layer } from "effect";

import {
	CodexEngine,
	CodexEngineDescriptor,
	CodexProcessFactoryLive,
	make_codex_engine_layer,
} from "@artisan/engines";

const fixture_path = fileURLToPath(new URL("./fixtures/fake-app-server.ts", import.meta.url));
const original_scenario = process.env.FAKE_APP_SERVER_SCENARIO;

afterEach(() => {
	if (original_scenario === undefined) {
		delete process.env.FAKE_APP_SERVER_SCENARIO;

		return;
	}

	process.env.FAKE_APP_SERVER_SCENARIO = original_scenario;
});

function make_layer(options: { readonly initialize_timeout_ms?: number } = {}) {
	return make_codex_engine_layer({
		...options,
		executable: process.execPath,
		executable_args: [fixture_path],
		initialize_timeout_ms: options.initialize_timeout_ms ?? 5_000,
		request_timeout_ms: 5_000,
		transport_selection: "app_server_only",
	}).pipe(Layer.provide(CodexProcessFactoryLive));
}

describe("Codex engine probe", () => {
	it("uses version, handshake, and account/read without starting a billable turn", async () => {
		const probe = await Effect.runPromise(
			Effect.gen(function* () {
				const engine = yield* CodexEngine;

				return yield* engine.Probe({
					client_name: "artisan-test",
					client_version: "0.3.0",
				});
			}).pipe(Effect.provide(make_layer())),
		);

		expect(probe.version).toBe("0.142.5");
		expect(probe.authentication).toMatchObject({ reason: "chatgpt", state: "authenticated" });
		expect(probe.ready).toBe(true);
		expect(CodexEngineDescriptor.capabilities.start.state).toBe("supported");
	});

	it("treats a configured Amazon Bedrock account as authenticated", async () => {
		process.env.FAKE_APP_SERVER_SCENARIO = "bedrock";

		const probe = await Effect.runPromise(
			Effect.gen(function* () {
				const engine = yield* CodexEngine;

				return yield* engine.Probe({});
			}).pipe(Effect.provide(make_layer())),
		);

		expect(probe.authentication).toEqual({
			reason: "amazonBedrock",
			state: "authenticated",
		});
		expect(probe.ready).toBe(true);
	});

	it("parses a version fragmented across stdout chunks", async () => {
		process.env.FAKE_APP_SERVER_SCENARIO = "version-fragmented";

		const probe = await Effect.runPromise(
			Effect.gen(function* () {
				const engine = yield* CodexEngine;

				return yield* engine.Probe({});
			}).pipe(Effect.provide(make_layer())),
		);

		expect(probe.version).toBe("0.142.5");
	});

	it.each(["stdout", "stderr"] as const)(
		"bounds app-server version %s before transport selection",
		async (channel) => {
			process.env.FAKE_APP_SERVER_SCENARIO = `version-${channel}-overflow`;

			await expect(
				Effect.runPromise(
					Effect.gen(function* () {
						const engine = yield* CodexEngine;

						return yield* engine.Probe({});
					}).pipe(Effect.provide(make_layer())),
				),
			).rejects.toMatchObject({
				_tag: "EngineProtocolError",
				message: `Codex --version ${channel} exceeded 65536 bytes`,
			});
		},
	);

	it("reports a distinct initialize timeout and closes the stalled app-server", async () => {
		process.env.FAKE_APP_SERVER_SCENARIO = "stall-initialize";

		await expect(
			Effect.runPromise(
				Effect.gen(function* () {
					const engine = yield* CodexEngine;

					return yield* engine.Probe({});
				}).pipe(Effect.provide(make_layer({ initialize_timeout_ms: 25 }))),
			),
		).rejects.toMatchObject({
			_tag: "EngineProbeTimeoutError",
			phase: "initialize",
			timeout_ms: 25,
		});
	});

	const live_it = process.env.ARTISAN_ENGINE_LIVE === "1" ? it : it.skip;

	live_it("performs only the live version, handshake, and account/read smoke check", async () => {
		const probe = await Effect.runPromise(
			Effect.gen(function* () {
				const engine = yield* CodexEngine;

				return yield* engine.Probe({
					client_name: "artisan-engine-smoke",
					client_version: "0.3.0",
				});
			}).pipe(
				Effect.provide(
					make_codex_engine_layer().pipe(Layer.provide(CodexProcessFactoryLive)),
				),
			),
		);

		expect(probe.version).toBe("0.142.5");
	});
});
