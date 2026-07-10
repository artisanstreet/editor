import { describe, expect, it } from "vitest";
import { Effect, Layer } from "effect";

import {
	CodexProcessFactory,
	CodexProcessFactoryLive,
	make_codex_engine,
	type CodexProcessHandle,
} from "@artisan/engines";

import { run_engine_inspection_scenario } from "./conformance";

const encoder = new TextEncoder();
const snowman = String.fromCodePoint(0x2603);

function chunks_from_text(text: string, split_at: number) {
	const bytes = encoder.encode(text);

	return [bytes.subarray(0, split_at), bytes.subarray(split_at)];
}

function make_handle(chunks: ReadonlyArray<Uint8Array>, writes: Array<string>): CodexProcessHandle {
	return {
		Close: Effect.void,
		Exit: Effect.succeed({ code: 0, signal: null }),
		Kill: () => Effect.void,
		Stderr: (async function* () {})(),
		Stdout: (async function* () {
			for (const chunk of chunks) {
				yield chunk;
			}
		})(),
		Write: (chunk) =>
			Effect.sync(() => {
				writes.push(new TextDecoder().decode(chunk));
			}),
	};
}

describe("Codex engine inspection", () => {
	it("discovers the version and completes initialize through a fake process factory", async () => {
		const commands: Array<ReadonlyArray<string>> = [];
		const writes: Array<string> = [];
		const user_agent = `codex-cli/0.142.5 ${snowman}`;
		const initialize_response = `${JSON.stringify({
			id: 1,
			result: {
				codexHome: "C:\\Users\\Sander\\.codex",
				platformFamily: "windows",
				platformOs: "windows",
				userAgent: user_agent,
			},
		})}\n`;
		let spawn_count = 0;
		const process_layer = Layer.succeed(CodexProcessFactory, {
			Spawn: (input) =>
				Effect.sync(() => {
					commands.push(input.args);
					spawn_count += 1;

					return make_handle(
						spawn_count === 1
							? [encoder.encode("codex-cli 0.142.5\n")]
							: chunks_from_text(
									initialize_response,
									initialize_response.indexOf(snowman) + 1,
								),
						writes,
					);
				}),
		});

		await run_engine_inspection_scenario(
			{
				engine: make_codex_engine(),
				input: { client_name: "artisan-conformance", client_version: "0.1.0" },
				name: "version and initialize",
				verify: (inspection) => {
					expect(inspection.version).toBe("0.142.5");
					expect(inspection.metadata.user_agent).toBe(user_agent);
				},
			},
			process_layer,
		);

		expect(commands).toEqual([["--version"], ["app-server", "--stdio"]]);
		expect(JSON.parse(writes[0] ?? "{}")).toMatchObject({
			id: 1,
			method: "initialize",
			params: {
				clientInfo: { name: "artisan-conformance", version: "0.1.0" },
			},
		});
	});

	it("fails start and resume explicitly until the run slice exists", async () => {
		const engine = make_codex_engine();

		await expect(
			Effect.runPromise(
				engine
					.Start({ working_directory: "C:\\workspace" })
					.pipe(Effect.provide(CodexProcessFactoryLive)),
			),
		).rejects.toMatchObject({ _tag: "EngineUnsupportedOperationError", operation: "start" });
		await expect(
			Effect.runPromise(
				engine.Resume({ run_id: "run_1" }).pipe(Effect.provide(CodexProcessFactoryLive)),
			),
		).rejects.toMatchObject({ _tag: "EngineUnsupportedOperationError", operation: "resume" });
	});

	const live_it = process.env.ARTISAN_ENGINE_LIVE === "1" ? it : it.skip;

	live_it("performs only the live version and initialize smoke check", async () => {
		const inspection = await Effect.runPromise(
			make_codex_engine()
				.Inspect({ client_name: "artisan-engine-smoke", client_version: "0.1.0" })
				.pipe(Effect.provide(CodexProcessFactoryLive)),
		);

		expect(inspection.version).toBe("0.142.5");
	});
});
