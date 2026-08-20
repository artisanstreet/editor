import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { afterEach, describe, expect, it } from "vitest";
import { Deferred, Effect, Exit, Fiber, Layer, Ref, Stream } from "effect";

import {
	ClaudeCliEngine,
	make_claude_cli_engine_layer,
} from "../../modules/engines/src/claude/cli-engine";
import { EngineProcessFactoryLive } from "../../modules/engines/src/process/process";

const executable = fileURLToPath(new URL("./fixtures/fake-claude.ts", import.meta.url));
const original_scenario = process.env.FAKE_CLAUDE_SCENARIO;
const original_invocation_file = process.env.FAKE_CLAUDE_INVOCATION_FILE;
const original_grandchild_file = process.env.FAKE_CLAUDE_GRANDCHILD_PID_FILE;

afterEach(() => {
	if (original_scenario === undefined) delete process.env.FAKE_CLAUDE_SCENARIO;
	else process.env.FAKE_CLAUDE_SCENARIO = original_scenario;
	if (original_invocation_file === undefined) delete process.env.FAKE_CLAUDE_INVOCATION_FILE;
	else process.env.FAKE_CLAUDE_INVOCATION_FILE = original_invocation_file;
	if (original_grandchild_file === undefined) delete process.env.FAKE_CLAUDE_GRANDCHILD_PID_FILE;
	else process.env.FAKE_CLAUDE_GRANDCHILD_PID_FILE = original_grandchild_file;
});

const IsProcessAlive = (pid: number) => {
	try {
		process.kill(pid, 0);
		return true;
	} catch {
		return false;
	}
};

const WaitFor = async (predicate: () => boolean, description: string) => {
	const deadline = Date.now() + 2_000;
	while (!predicate()) {
		if (Date.now() >= deadline) throw new Error(`Timed out waiting for ${description}`);
		await new Promise<void>((resolve) => setTimeout(resolve, 10));
	}
};

const GetEngine = (options: Parameters<typeof make_claude_cli_engine_layer>[0] = {}) =>
	Effect.runPromise(
		ClaudeCliEngine.pipe(
			Effect.provide(
				make_claude_cli_engine_layer({
					executable: process.execPath,
					executable_args: [executable],
					...options,
				}).pipe(Layer.provide(EngineProcessFactoryLive)),
			),
		),
	);

const Collect = async (
	input: Parameters<Awaited<ReturnType<typeof GetEngine>>["Open"]>[0],
	options: Parameters<typeof GetEngine>[0] = {},
) => {
	const engine = await GetEngine(options);
	return Effect.runPromise(
		Effect.scoped(
			Effect.gen(function* () {
				const run = yield* engine.Open(input);
				return [...(yield* run.Events.pipe(Stream.runCollect))];
			}),
		),
	);
};

const StartInput = (overrides: Record<string, unknown> = {}) => ({
	_tag: "start" as const,
	artisan_run_id: "cli-run",
	initial_text: "hello",
	working_directory: process.cwd(),
	...overrides,
});

describe("Claude direct CLI transport", () => {
	it("is an isolated opt-in transport with no Agent SDK dependency", () => {
		expect(ClaudeCliEngine.key).toBe("Artisan/ClaudeEngine");
		expect(make_claude_cli_engine_layer({ executable: "claude" })).toBeDefined();
	});

	it("keeps direct CLI stdin interactive for approval and steering, then settles one result", async () => {
		process.env.FAKE_CLAUDE_SCENARIO = "interactive";
		const engine = await Effect.runPromise(
			ClaudeCliEngine.pipe(
				Effect.provide(
					make_claude_cli_engine_layer({
						executable: process.execPath,
						executable_args: [executable],
					}).pipe(Layer.provide(EngineProcessFactoryLive)),
				),
			),
		);
		const events = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const run = yield* engine.Open({
						_tag: "start",
						artisan_run_id: "cli-interactive",
						initial_text: "start",
						working_directory: process.cwd(),
					});
					const approval_requested = yield* Deferred.make<void>();
					const observed = yield* Ref.make<ReadonlyArray<unknown>>([]);
					const collector = yield* Effect.forkScoped(
						run.Events.pipe(
							Stream.runForEach((event) =>
								Ref.update(observed, (events) => [...events, event]).pipe(
									Effect.andThen(
										event._tag === "approval" && event.state === "requested"
											? Deferred.succeed(approval_requested, undefined)
											: Effect.void,
									),
								),
							),
						),
					);
					yield* Deferred.await(approval_requested);
					yield* run.Send({
						_tag: "respond_approval",
						approved: true,
						approval_id: "permission-1",
						command_id: "approve",
					});
					yield* run.Send({ _tag: "steer", command_id: "steer", text: "continue" });
					yield* Fiber.join(collector);
					return yield* Ref.get(observed);
				}),
			),
		);
		expect(events).toContainEqual(
			expect.objectContaining({
				_tag: "approval",
				approval_id: "permission-1",
				state: "requested",
			}),
		);
		expect(events).toContainEqual(
			expect.objectContaining({
				_tag: "approval",
				approval_id: "permission-1",
				state: "resolved",
			}),
		);
		expect(events).toContainEqual(
			expect.objectContaining({
				_tag: "subagent_transcript",
				agent_native_thread_id: "child-task",
			}),
		);
		expect(events.at(-1)).toMatchObject({ _tag: "run_terminal", state: "completed" });
	});

	/**
	 * A question is not a permission: answering it is the authorization. Routing
	 * AskUserQuestion through the approval path put a prompt in front of a tool
	 * that can only ask something, and the CLI then blocked on a terminal dialog
	 * Artisan never renders — the user cleared a prompt and nothing happened.
	 */
	it("canonicalizes AskUserQuestion as answerable questions instead of an approval", async () => {
		process.env.FAKE_CLAUDE_SCENARIO = "interactive-question";
		const engine = await GetEngine();
		const events = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const run = yield* engine.Open({
						_tag: "start",
						artisan_run_id: "cli-question",
						initial_text: "start",
						working_directory: process.cwd(),
					});
					const asked = yield* Deferred.make<void>();
					const observed = yield* Ref.make<ReadonlyArray<unknown>>([]);
					const collector = yield* Effect.forkScoped(
						run.Events.pipe(
							Stream.runForEach((event) =>
								Ref.update(observed, (events) => [...events, event]).pipe(
									Effect.andThen(
										event._tag === "question" &&
											event.state === "requested" &&
											event.question_id === "question-1:1"
											? Deferred.succeed(asked, undefined)
											: Effect.void,
									),
								),
							),
						),
					);
					yield* Deferred.await(asked);
					/**
					 * Each question is its own card, so answers arrive one at a time.
					 * The response must not be released until the request's whole group
					 * is answered — the fixture fails the turn if it is.
					 */
					yield* run.Send({
						_tag: "respond_question",
						answers: { "question-1:0": ["Effect"] },
						command_id: "answer-one",
					});
					yield* run.Send({
						_tag: "respond_question",
						answers: { "question-1:1": ["Desktop", "Web"] },
						command_id: "answer-two",
					});
					yield* Fiber.join(collector);
					return yield* Ref.get(observed);
				}),
			),
		);

		expect(events).not.toContainEqual(expect.objectContaining({ _tag: "approval" }));
		expect(events).toContainEqual(
			expect.objectContaining({
				_tag: "question",
				header: "Library",
				multi_select: false,
				options: [
					{ description: "Already in the workspace", label: "Effect" },
					{ description: "One more dependency", label: "RxJS" },
				],
				question_id: "question-1:0",
				state: "requested",
				text: "Which library should we use?",
			}),
		);
		expect(events).toContainEqual(
			expect.objectContaining({
				_tag: "question",
				multi_select: true,
				question_id: "question-1:1",
				state: "requested",
			}),
		);
		expect(events).toContainEqual(
			expect.objectContaining({
				_tag: "question",
				answers: ["Desktop", "Web"],
				question_id: "question-1:1",
				state: "resolved",
			}),
		);
		/** The fixture only succeeds when both answers arrive in one allow response. */
		expect(events.at(-1)).toMatchObject({ _tag: "run_terminal", state: "completed" });
	});

	it("rejects an answer to a question the run never asked", async () => {
		process.env.FAKE_CLAUDE_SCENARIO = "interactive-question";
		const engine = await GetEngine();
		const exit = await Effect.runPromiseExit(
			Effect.scoped(
				Effect.gen(function* () {
					const run = yield* engine.Open({
						_tag: "start",
						artisan_run_id: "cli-question-unknown",
						initial_text: "start",
						working_directory: process.cwd(),
					});
					const asked = yield* Deferred.make<void>();
					yield* Effect.forkScoped(
						run.Events.pipe(
							Stream.runForEach((event) =>
								event._tag === "question" && event.state === "requested"
									? Deferred.succeed(asked, undefined)
									: Effect.void,
							),
						),
					);
					yield* Deferred.await(asked);
					return yield* run.Send({
						_tag: "respond_question",
						answers: { "question-9:0": ["Effect"] },
						command_id: "answer-unknown",
					});
				}),
			),
		);

		expect(Exit.isFailure(exit)).toBe(true);
	});

	it("writes the exact deny control envelope before accepting further steering", async () => {
		process.env.FAKE_CLAUDE_SCENARIO = "interactive-deny";
		const engine = await Effect.runPromise(
			ClaudeCliEngine.pipe(
				Effect.provide(
					make_claude_cli_engine_layer({
						executable: process.execPath,
						executable_args: [executable],
					}).pipe(Layer.provide(EngineProcessFactoryLive)),
				),
			),
		);
		const events = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const run = yield* engine.Open({
						_tag: "start",
						artisan_run_id: "cli-interactive-deny",
						initial_text: "start",
						working_directory: process.cwd(),
					});
					const approval_requested = yield* Deferred.make<void>();
					const observed = yield* Ref.make<ReadonlyArray<unknown>>([]);
					const collector = yield* Effect.forkScoped(
						run.Events.pipe(
							Stream.runForEach((event) =>
								Ref.update(observed, (events) => [...events, event]).pipe(
									Effect.andThen(
										event._tag === "approval" && event.state === "requested"
											? Deferred.succeed(approval_requested, undefined)
											: Effect.void,
									),
								),
							),
						),
					);
					yield* Deferred.await(approval_requested);
					yield* run.Send({
						_tag: "respond_approval",
						approved: false,
						approval_id: "permission-1",
						command_id: "deny",
					});
					yield* run.Send({ _tag: "steer", command_id: "steer", text: "continue" });
					yield* Fiber.join(collector);
					return yield* Ref.get(observed);
				}),
			),
		);
		expect(events).toContainEqual(
			expect.objectContaining({
				_tag: "approval",
				approval_id: "permission-1",
				approved: false,
				state: "resolved",
			}),
		);
		expect(events.at(-1)).toMatchObject({ _tag: "run_terminal", state: "completed" });
	});

	it("rejects unknown/duplicate approvals and conflicting duplicate command ids", async () => {
		process.env.FAKE_CLAUDE_SCENARIO = "interactive";
		const engine = await GetEngine();
		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const run = yield* engine.Open(
						StartInput({ artisan_run_id: "cli-command-errors" }),
					);
					const approval_requested = yield* Deferred.make<void>();
					const collector = yield* Effect.forkScoped(
						run.Events.pipe(
							Stream.runForEach((event) =>
								event._tag === "approval" && event.state === "requested"
									? Deferred.succeed(approval_requested, undefined)
									: Effect.void,
							),
						),
					);
					yield* Deferred.await(approval_requested);
					const unknown = yield* Effect.exit(
						run.Send({
							_tag: "respond_approval",
							approved: true,
							approval_id: "unknown",
							command_id: "unknown",
						}),
					);
					expect(Exit.isFailure(unknown)).toBe(true);
					yield* run.Send({
						_tag: "respond_approval",
						approved: true,
						approval_id: "permission-1",
						command_id: "approve",
					});
					const duplicate_target = yield* Effect.exit(
						run.Send({
							_tag: "respond_approval",
							approved: true,
							approval_id: "permission-1",
							command_id: "approve-again",
						}),
					);
					expect(Exit.isFailure(duplicate_target)).toBe(true);
					yield* run.Send({ _tag: "steer", command_id: "steer", text: "continue" });
					const conflict = yield* Effect.exit(
						run.Send({ _tag: "steer", command_id: "steer", text: "different" }),
					);
					expect(Exit.isFailure(conflict)).toBe(true);
					yield* Fiber.join(collector);
				}),
			),
		);
	});

	it.each([
		"malformed",
		"oversized",
		"semantic-failure",
		"missing-init",
		"missing-result",
		"nonzero",
		"mismatch",
		"stderr-bounds",
	])("settles failed for immediate provider %s failures", async (scenario) => {
		process.env.FAKE_CLAUDE_SCENARIO = `immediate-${scenario}`;
		const events = await Collect(StartInput({ artisan_run_id: `cli-${scenario}` }), {
			...(scenario === "oversized" ? { max_frame_bytes: 128 } : {}),
			...(scenario === "stderr-bounds" ? { max_stderr_bytes: 128 } : {}),
		});
		expect(events.at(-1)).toMatchObject({ _tag: "run_terminal", state: "failed" });
	});

	it("uses fresh/resumed session flags, preserves ordered content, and does not invent empty resume input", async () => {
		const directory = mkdtempSync(join(tmpdir(), "artisan-claude-cli-"));
		const invocation_file = join(directory, "invocations.jsonl");
		try {
			process.env.FAKE_CLAUDE_INVOCATION_FILE = invocation_file;
			process.env.FAKE_CLAUDE_SCENARIO = "immediate-result";
			await Collect(
				StartInput({
					model: "fake-model",
					product_instructions: {
						content: "Follow Artisan policy.",
						source: "artisan-product",
					},
					provider_options: {
						"claude.append_system_prompt_file": "C:\\workspace\\CLAUDE.md",
						"claude.disable_tools": true,
						"claude.effort": "max",
						"claude.permission_mode": "plan",
						"claude.safe_mode": true,
					},
					initial_content: [
						{ type: "text", text: "before" },
						{
							type: "image",
							id: "image-1",
							name: "shot.png",
							media_type: "image/png",
							bytes: new Uint8Array([1, 2, 3]),
						},
						{ type: "text", text: "after" },
					],
				}),
			);
			const start_records = readFileSync(invocation_file, "utf8")
				.trim()
				.split("\n")
				.map((line) => JSON.parse(line) as Record<string, unknown>);
			const start = start_records.find(
				(record) => Array.isArray(record.args) && record.args.includes("-p"),
			)!;
			expect(start.args).toEqual(
				expect.arrayContaining([
					"--session-id",
					"-p",
					/**
					 * A `-p` session is forced to `omitted` unless the flag is explicit,
					 * which is what made every `thinking_delta` arrive empty beside a
					 * signature — reasoning nobody had asked for rather than reasoning
					 * the provider withholds.
					 */
					"--thinking-display",
					"summarized",
					"--permission-mode",
					"plan",
					"--tools",
					"",
					"--safe-mode",
					"--effort",
					"max",
					"--append-system-prompt-file",
					"C:\\workspace\\CLAUDE.md",
					"--append-system-prompt",
					"Follow Artisan policy.",
					"--model",
					"fake-model",
				]),
			);
			const start_stdin = start_records
				.filter((record) => typeof record.stdin_chunk === "string")
				.map((record) => record.stdin_chunk)
				.join("");
			expect(JSON.parse(start_stdin)).toMatchObject({
				message: {
					content: [
						{ type: "text", text: "before" },
						{
							type: "image",
							source: { type: "base64", media_type: "image/png", data: "AQID" },
						},
						{ type: "text", text: "after" },
					],
				},
			});

			process.env.FAKE_CLAUDE_SCENARIO = "immediate-empty-resume";
			await Collect({
				_tag: "resume",
				artisan_run_id: "cli-empty-resume",
				working_directory: process.cwd(),
				resume_token: { native_thread_id: "resume-session" },
			});
			const all_records = readFileSync(invocation_file, "utf8")
				.trim()
				.split("\n")
				.map((line) => JSON.parse(line) as Record<string, unknown>);
			const resume = all_records.filter((record) => Array.isArray(record.args)).at(-1)!;
			expect(resume.args).toEqual(expect.arrayContaining(["--resume", "resume-session"]));
			expect(all_records.filter((record) => "stdin_chunk" in record)).toHaveLength(1);
		} finally {
			rmSync(directory, { force: true, recursive: true });
		}
	});

	it("retries a healing authentication probe before starting the real process", async () => {
		const directory = mkdtempSync(join(tmpdir(), "artisan-claude-auth-"));
		try {
			process.env.FAKE_CLAUDE_INVOCATION_FILE = join(directory, "invocations.jsonl");
			process.env.FAKE_CLAUDE_SCENARIO = "immediate-auth-heals";
			const events = await Collect(StartInput({ artisan_run_id: "cli-auth-heals" }), {
				auth_retry_delay_ms: 1,
			});
			expect(events.at(-1)).toMatchObject({ _tag: "run_terminal", state: "completed" });
		} finally {
			rmSync(directory, { force: true, recursive: true });
		}
	});

	it("times out an inactive real process and emits exactly one failed terminal", async () => {
		process.env.FAKE_CLAUDE_SCENARIO = "immediate-inactivity";
		const events = await Collect(StartInput({ artisan_run_id: "cli-inactivity" }), {
			inactivity_ms: 20,
		});
		expect(
			events.filter((event) => (event as { _tag?: string })._tag === "run_terminal"),
		).toEqual([expect.objectContaining({ state: "failed" })]);
	});

	it.each([
		["cancel", "cancelled"],
		["close", "closed"],
	] as const)(
		"%s terminates the real provider process tree exactly once",
		async (command, state) => {
			const directory = mkdtempSync(join(tmpdir(), "artisan-claude-tree-"));
			const pid_file = join(directory, "pids.json");
			try {
				process.env.FAKE_CLAUDE_SCENARIO = "immediate-cancel-tree";
				process.env.FAKE_CLAUDE_GRANDCHILD_PID_FILE = pid_file;
				const engine = await GetEngine();
				const events = await Effect.runPromise(
					Effect.scoped(
						Effect.gen(function* () {
							const run = yield* engine.Open(
								StartInput({ artisan_run_id: `cli-${command}-tree` }),
							);
							const initialized = yield* Deferred.make<void>();
							const observed = yield* Ref.make<ReadonlyArray<unknown>>([]);
							const collector = yield* Effect.forkScoped(
								run.Events.pipe(
									Stream.runForEach((event) =>
										Ref.update(observed, (events) => [...events, event]).pipe(
											Effect.andThen(
												event._tag === "run_state" &&
													event.state === "running"
													? Deferred.succeed(initialized, undefined)
													: Effect.void,
											),
										),
									),
								),
							);
							yield* Deferred.await(initialized);
							yield* run.Send({ _tag: command, command_id: `${command}-tree` });
							yield* Fiber.join(collector);
							return yield* Ref.get(observed);
						}),
					),
				);
				expect(
					events.filter((event) => (event as { _tag?: string })._tag === "run_terminal"),
				).toEqual([expect.objectContaining({ state })]);
				await WaitFor(() => existsSync(pid_file), "fixture process PID record");
				const pids = JSON.parse(readFileSync(pid_file, "utf8")) as {
					readonly grandchild_pid: number;
					readonly provider_pid: number;
				};
				await WaitFor(
					() =>
						!IsProcessAlive(pids.provider_pid) && !IsProcessAlive(pids.grandchild_pid),
					"provider process tree termination",
				);
			} finally {
				rmSync(directory, { force: true, recursive: true });
			}
		},
	);
});
