import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Cause, Effect, Exit, Layer, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { CodexEngine, EngineProcessFactory, make_codex_engine_layer } from "@artisan/engines";
import { MakeCodexAppServerThreadOptions } from "../../modules/engines/src/codex/internal/permissions";
import { EngineProcessFactoryLive } from "../../modules/engines/src/process/process";
import {
	ClaudeCliEngine,
	make_claude_cli_engine_layer,
} from "../../modules/engines/src/claude/cli-engine";

const app_server_fixture = fileURLToPath(new URL("./fixtures/fake-app-server.ts", import.meta.url));
const claude_fixture = fileURLToPath(new URL("./fixtures/fake-claude.ts", import.meta.url));

const start_input = {
	_tag: "start" as const,
	artisan_run_id: "product-instructions-run",
	initial_text: "Preserve this user text exactly.",
	working_directory: process.cwd(),
};

const product_instructions = {
	content: "Follow Artisan product policy.",
	source: "artisan-product",
};
const original_claude_scenario = process.env.FAKE_CLAUDE_SCENARIO;
const original_claude_invocation_file = process.env.FAKE_CLAUDE_INVOCATION_FILE;

afterEach(() => {
	if (original_claude_scenario === undefined) delete process.env.FAKE_CLAUDE_SCENARIO;
	else process.env.FAKE_CLAUDE_SCENARIO = original_claude_scenario;
	if (original_claude_invocation_file === undefined)
		delete process.env.FAKE_CLAUDE_INVOCATION_FILE;
	else process.env.FAKE_CLAUDE_INVOCATION_FILE = original_claude_invocation_file;
});

function failure_from<A>(exit: Exit.Exit<A, unknown>) {
	return Exit.isFailure(exit) ? Cause.squash(exit.cause) : undefined;
}

describe("engine product instructions", () => {
	it("rejects blank product instructions before Codex app-server spawn", async () => {
		let spawn_count = 0;
		const factory = Layer.succeed(EngineProcessFactory, {
			Spawn: () => {
				spawn_count += 1;
				return Effect.die("must not spawn");
			},
		});
		const engine = await Effect.runPromise(
			CodexEngine.pipe(
				Effect.provide(
					make_codex_engine_layer({
						executable: process.execPath,
						executable_args: [app_server_fixture],
					}).pipe(Layer.provide(factory)),
				),
			),
		);
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.exit(
					engine.Open({
						...start_input,
						product_instructions: { ...product_instructions, content: " \n\t" },
					}),
				),
			),
		);

		expect(failure_from(result)).toMatchObject({
			_tag: "EngineConfigurationError",
			option: "product_instructions.content",
		});
		expect(spawn_count).toBe(0);
	});

	it("maps product and global instructions to labelled Codex app-server sections", async () => {
		const options = await Effect.runPromise(
			MakeCodexAppServerThreadOptions({
				...start_input,
				global_guidance: { content: "Keep the repository tidy.", source_file: "AGENTS.md" },
				product_instructions,
			}),
		);

		expect(options).toMatchObject({
			developerInstructions:
				"## User global guidance\n\nKeep the repository tidy.\n\n## Artisan product instructions\n\nFollow Artisan product policy.\n\nThe Artisan product instructions describe the active presentation surface and take precedence over conflicting output-format guidance above.",
		});
	});

	it("preserves the existing exact Codex global-guidance payload", async () => {
		const options = await Effect.runPromise(
			MakeCodexAppServerThreadOptions({
				...start_input,
				global_guidance: { content: "Keep the repository tidy.", source_file: "AGENTS.md" },
			}),
		);

		expect(options.developerInstructions).toBe("Keep the repository tidy.");
	});

	it("rejects blank product instructions before Claude starts a process", async () => {
		let spawn_count = 0;
		const factory = Layer.succeed(EngineProcessFactory, {
			Spawn: () => {
				spawn_count += 1;
				return Effect.die("must not spawn");
			},
		});
		const engine = await Effect.runPromise(
			ClaudeCliEngine.pipe(
				Effect.provide(make_claude_cli_engine_layer().pipe(Layer.provide(factory))),
			),
		);
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.exit(
					engine.Open({
						...start_input,
						product_instructions: { ...product_instructions, source: "\t" },
					}),
				),
			),
		);

		expect(failure_from(result)).toMatchObject({
			_tag: "EngineConfigurationError",
			option: "product_instructions.source",
		});
		expect(spawn_count).toBe(0);
	});

	it("appends one Claude CLI system prompt on start and resume without changing user text", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-claude-product-"));
		const invocation_file = join(directory, "invocations.jsonl");
		process.env.FAKE_CLAUDE_SCENARIO = "immediate-result";
		process.env.FAKE_CLAUDE_INVOCATION_FILE = invocation_file;
		const engine = await Effect.runPromise(
			ClaudeCliEngine.pipe(
				Effect.provide(
					make_claude_cli_engine_layer({
						executable: process.execPath,
						executable_args: [claude_fixture],
					}).pipe(Layer.provide(EngineProcessFactoryLive)),
				),
			),
		);
		try {
			for (const input of [
				{
					...start_input,
					product_instructions,
					provider_options: {
						"claude.append_system_prompt_file": "C:\\workspace\\CLAUDE.md",
					},
				},
				{
					_tag: "resume" as const,
					artisan_run_id: "product-instructions-resume",
					next_text: "Keep this resume text exact.",
					product_instructions,
					provider_options: {
						"claude.append_system_prompt_file": "C:\\workspace\\CLAUDE.md",
					},
					resume_token: { native_thread_id: "session" },
					working_directory: process.cwd(),
				},
			]) {
				await Effect.runPromise(
					Effect.scoped(
						Effect.gen(function* () {
							const run = yield* engine.Open(input);
							yield* run.Events.pipe(Stream.runDrain);
						}),
					),
				);
			}

			const records = (await readFile(invocation_file, "utf8"))
				.trim()
				.split("\n")
				.map(
					(line) =>
						JSON.parse(line) as { args?: ReadonlyArray<string>; stdin_chunk?: string },
				);
			const runs = records.filter((record) => record.args?.includes("-p"));
			expect(runs).toHaveLength(2);
			for (const run of runs) {
				expect(run.args).toEqual(
					expect.arrayContaining([
						"-p",
						"--output-format",
						"stream-json",
						"--input-format",
						"stream-json",
						"--append-system-prompt-file",
						"C:\\workspace\\CLAUDE.md",
						"--append-system-prompt",
						"Follow Artisan product policy.",
					]),
				);
			}
			expect(runs[0]?.args).toEqual(expect.arrayContaining(["--session-id"]));
			expect(runs[1]?.args).toEqual(expect.arrayContaining(["--resume", "session"]));
			const user_messages = records
				.flatMap((record) => (record.stdin_chunk === undefined ? [] : [record.stdin_chunk]))
				.map((chunk) => JSON.parse(chunk) as { message: { content: string } });
			expect(user_messages.map((message) => message.message.content)).toEqual([
				"Preserve this user text exactly.",
				"Keep this resume text exact.",
			]);
		} finally {
			await rm(directory, { force: true, recursive: true });
		}
	});
});
