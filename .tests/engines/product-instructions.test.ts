import { fileURLToPath } from "node:url";

import { Cause, Effect, Exit, Layer, Stream } from "effect";
import { describe, expect, it } from "vitest";

import {
	ClaudeEngine,
	CodexEngine,
	EngineProcessFactory,
	EngineProcessFactoryLive,
	make_claude_engine_layer,
	make_codex_engine_layer,
} from "@artisan/engines";
import type { SDKMessage } from "@anthropic-ai/claude-agent-sdk";
import { MakeCodexAppServerThreadOptions } from "../../modules/engines/src/codex/internal/permissions";
import {
	make_fake_claude_query,
	make_unstartable_claude_query,
	type FakeClaudeSession,
} from "./fixtures/fake-claude-query";

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
				Effect.provide(
					make_claude_engine_layer().pipe(
						Layer.provide(factory),
						Layer.provide(make_unstartable_claude_query()),
					),
				),
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

	it("appends one Claude system prompt on start and resume without changing user text", async () => {
		const fake = make_fake_claude_query((session: FakeClaudeSession) => {
			session.emit({
				type: "system",
				subtype: "init",
				cwd: process.cwd(),
				session_id: session.session_id(),
				tools: [],
				model: "claude-haiku-4-5",
				permissionMode: "default",
				uuid: "3f0d9f4a-2c3d-4b0e-9f70-5f6f2f9a1b22",
			} as unknown as SDKMessage);
			session.emit({
				type: "result",
				subtype: "success",
				is_error: false,
				num_turns: 1,
				session_id: session.session_id(),
				uuid: "3d8a2f5c-0b47-4a3e-9a3e-6c2f6f4d5a01",
			} as unknown as SDKMessage);
		});
		const engine = await Effect.runPromise(
			ClaudeEngine.pipe(
				Effect.provide(
					make_claude_engine_layer({
						executable: process.execPath,
						executable_args: [claude_fixture],
					}).pipe(Layer.provide(EngineProcessFactoryLive), Layer.provide(fake.layer)),
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

		expect(fake.sessions).toHaveLength(2);
		for (const session of fake.sessions) {
			expect(session.options?.systemPrompt).toEqual({
				append: "Follow Artisan product policy.",
				preset: "claude_code",
				type: "preset",
			});
			expect(session.options?.extraArgs).toEqual({
				"append-system-prompt-file": "C:\\workspace\\CLAUDE.md",
			});
		}
		expect(fake.sessions[0]?.received[0]?.message.content).toBe(
			"Preserve this user text exactly.",
		);
		expect(fake.sessions[1]?.received[0]?.message.content).toBe("Keep this resume text exact.");
	});
});
