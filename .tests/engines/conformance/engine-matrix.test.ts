import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

import { afterEach, describe, it } from "vitest";
import { Effect, Layer, Schema } from "effect";

import { CodexEngine, CodexProcessFactoryLive, make_codex_engine_layer } from "@artisan/engines";

import { make_fake_engine } from "../harness/fake-engine";
import { make_transcript_sequence_replay } from "../harness/transcript-process";
import { EngineOpenScenarios } from "../scenarios/engine-scenarios";
import { EngineTranscriptSequence } from "../transcript";
import { assert_engine_lifecycle_contract } from "./engine-lifecycle-contract";

const fixture_path = fileURLToPath(new URL("../fixtures/fake-app-server.ts", import.meta.url));
const transcript_path = new URL("../fixtures/transcripts/engine-lifecycle.json", import.meta.url);
const original_scenario = process.env.FAKE_APP_SERVER_SCENARIO;

const codex_open_input = {
	_tag: "resume" as const,
	artisan_run_id: "codex-shared-lifecycle",
	resume_token: {
		native_thread_id: "native-thread-resume",
		opaque_checkpoint: "codex-checkpoint",
	},
	working_directory: "C:\\workspace",
};

afterEach(() => {
	if (original_scenario === undefined) {
		delete process.env.FAKE_APP_SERVER_SCENARIO;
	} else {
		process.env.FAKE_APP_SERVER_SCENARIO = original_scenario;
	}
});

describe("Shared Engine lifecycle contract", () => {
	it("passes through the deterministic in-memory adapter", async () => {
		await assert_engine_lifecycle_contract(make_fake_engine(), EngineOpenScenarios.resume);
	});

	it("passes through the Codex app-server process adapter", async () => {
		process.env.FAKE_APP_SERVER_SCENARIO = "resume-active";

		const engine = await Effect.runPromise(
			CodexEngine.pipe(
				Effect.provide(
					make_codex_engine_layer({
						executable: process.execPath,
						executable_args: [fixture_path],
						transport_selection: "app_server_only",
					}).pipe(Layer.provide(CodexProcessFactoryLive)),
				),
			),
		);

		await assert_engine_lifecycle_contract(engine, codex_open_input);
	}, 15_000);

	it("passes through the recorded Codex process transcript", async () => {
		const source = await readFile(transcript_path, "utf8");
		const recorded = await Effect.runPromise(
			Schema.decodeUnknownEffect(EngineTranscriptSequence)(JSON.parse(source)),
		);
		const portable = recorded.map((record) => ({
			...record,
			args: record.args.map((argument) =>
				argument === "{{FAKE_APP_SERVER}}" || argument.endsWith("fake-app-server.ts")
					? fixture_path
					: argument,
			),
			command: process.execPath,
		}));
		const replay = await Effect.runPromise(make_transcript_sequence_replay(portable));
		const engine = await Effect.runPromise(
			CodexEngine.pipe(
				Effect.provide(
					make_codex_engine_layer({
						executable: process.execPath,
						executable_args: [fixture_path],
						transport_selection: "app_server_only",
					}).pipe(Layer.provide(replay.Layer)),
				),
			),
		);

		await assert_engine_lifecycle_contract(engine, codex_open_input);
		await Effect.runPromise(replay.Assert);
		await Effect.runPromise(replay.AssertClosed);
	});
});
