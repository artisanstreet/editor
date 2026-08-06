import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Cause, Effect, Exit, Layer, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	ClaudeEngine,
	CodexEngine,
	EngineProcessFactory,
	EngineProcessFactoryLive,
	make_claude_engine_layer,
	make_codex_engine_layer,
} from "@artisan/engines";
import { MakeCodexAppServerThreadOptions } from "../../modules/engines/src/codex/internal/permissions";

const app_server_fixture = fileURLToPath(new URL("./fixtures/fake-app-server.ts", import.meta.url));
const claude_fixture = fileURLToPath(new URL("./fixtures/fake-claude.ts", import.meta.url));
const original_claude_invocation = process.env.FAKE_CLAUDE_INVOCATION_FILE;

afterEach(() => {
	if (original_claude_invocation === undefined) delete process.env.FAKE_CLAUDE_INVOCATION_FILE;
	else process.env.FAKE_CLAUDE_INVOCATION_FILE = original_claude_invocation;
});

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
			ClaudeEngine.pipe(
				Effect.provide(make_claude_engine_layer().pipe(Layer.provide(factory))),
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

	it("adds one Claude system prompt on start and resume without changing stream-json stdin", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-product-instructions-"));
		const invocation = join(directory, "invocations.jsonl");
		process.env.FAKE_CLAUDE_INVOCATION_FILE = invocation;

		try {
			const engine = await Effect.runPromise(
				ClaudeEngine.pipe(
					Effect.provide(
						make_claude_engine_layer({
							executable: process.execPath,
							executable_args: [claude_fixture],
						}).pipe(Layer.provide(EngineProcessFactoryLive)),
					),
				),
			);
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
			const records = (await readFile(invocation, "utf8"))
				.trim()
				.split("\n")
				.map(
					(line) =>
						JSON.parse(line) as {
							readonly args?: ReadonlyArray<string>;
							readonly stdin?: string;
						},
				);
			const runs = records.filter((record) => record.args?.includes("-p") === true);
			const stdin = records.flatMap((record) =>
				record.stdin === undefined ? [] : [record.stdin],
			);

			expect(runs).toHaveLength(2);
			for (const run of runs) {
				expect(run.args?.filter((arg) => arg === "--append-system-prompt")).toHaveLength(1);
				expect(run.args).toContain("Follow Artisan product policy.");
				expect(
					run.args?.filter((arg) => arg === "--append-system-prompt-file"),
				).toHaveLength(1);
				expect(run.args).toContain("C:\\workspace\\CLAUDE.md");
			}
			expect(JSON.parse(stdin[0]!).message.content).toBe("Preserve this user text exactly.");
			expect(JSON.parse(stdin[1]!).message.content).toBe("Keep this resume text exact.");
		} finally {
			await rm(directory, { force: true, recursive: true });
		}
	});
});
