import { readFile, mkdtemp, rm } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { join } from "node:path";

import { describe, expect, it, afterEach } from "vitest";
import { Effect, Exit, Layer, Stream } from "effect";

import {
	ClaudeEngine,
	EngineRegistry,
	ClaudeEngineDescriptor,
	EngineProcessFactory,
	EngineProcessFactoryLive,
	make_engine_registry_layer,
	make_claude_engine_layer,
} from "@artisan/engines";

const executable = fileURLToPath(new URL("./fixtures/fake-claude.mjs", import.meta.url));
const original_scenario = process.env.FAKE_CLAUDE_SCENARIO;
const original_invocation = process.env.FAKE_CLAUDE_INVOCATION_FILE;

afterEach(() => {
	if (original_scenario === undefined) delete process.env.FAKE_CLAUDE_SCENARIO;
	else process.env.FAKE_CLAUDE_SCENARIO = original_scenario;
	if (original_invocation === undefined) delete process.env.FAKE_CLAUDE_INVOCATION_FILE;
	else process.env.FAKE_CLAUDE_INVOCATION_FILE = original_invocation;
});

function get_engine(options: Record<string, unknown> = {}) {
	return Effect.runPromise(
		ClaudeEngine.pipe(
			Effect.provide(
				make_claude_engine_layer({
					executable: process.execPath,
					executable_args: [executable],
					...options,
				}).pipe(Layer.provide(EngineProcessFactoryLive)),
			),
		),
	);
}

const open_input = (tag: "start" | "resume" = "start") =>
	tag === "start"
		? {
				_tag: "start" as const,
				artisan_run_id: "claude-run",
				initial_text: "hello",
				working_directory: process.cwd(),
				model: "fake-model",
				provider_options: { "claude.permission_mode": "default" },
			}
		: {
				_tag: "resume" as const,
				artisan_run_id: "claude-resume",
				next_text: "continue",
				working_directory: process.cwd(),
				resume_token: { native_thread_id: "resume-session" },
			};

describe("Claude Code engine", () => {
	it("declares truthful V1 capabilities", () => {
		expect(ClaudeEngineDescriptor.capabilities.steer.state).toBe("unsupported");
		expect(ClaudeEngineDescriptor.capabilities.approval.state).toBe("unsupported");
		expect(ClaudeEngineDescriptor.capabilities.resume.state).toBe("supported");
	});

	it("is explicitly registrable by its provider id", async () => {
		const engine = await get_engine();
		const resolved = await Effect.runPromise(
			Effect.gen(function* () {
				const registry = yield* EngineRegistry;
				return yield* registry.Get("claude");
			}).pipe(Effect.provide(make_engine_registry_layer([engine]))),
		);
		expect(resolved.Descriptor.id).toBe("claude");
	});

	it("probes saved auth and sends exact start argv plus stream input", async () => {
		const directory = await mkdtemp(join(process.cwd(), ".claude-test-"));
		const invocation = join(directory, "invocations.jsonl");
		process.env.FAKE_CLAUDE_INVOCATION_FILE = invocation;

		try {
			const engine = await get_engine();
			const result = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const run = yield* engine.Open(open_input());
						return {
							events: yield* run.Events.pipe(Stream.runCollect),
							native_thread_id: run.native_thread_id,
						};
					}),
				),
			);
			const events = result.events;
			const records = (await readFile(invocation, "utf8"))
				.trim()
				.split("\n")
				.map((line) => JSON.parse(line));
			const run_args = records.findLast((record) => record.args)?.args as string[];
			const stdin = records.findLast((record) => record.stdin)?.stdin as string;

			expect(result.native_thread_id).toMatch(/^[0-9a-f-]{36}$/);
			expect(run_args).toEqual(
				expect.arrayContaining([
					"-p",
					"--output-format",
					"stream-json",
					"--input-format",
					"stream-json",
					"--verbose",
					"--include-partial-messages",
					"--model",
					"fake-model",
					"--permission-mode",
					"default",
				]),
			);
			expect(JSON.parse(stdin).message.content).toBe("hello");
			expect(events[events.length - 1]).toMatchObject({
				_tag: "run_terminal",
				state: "completed",
			});
			expect(events.filter((event) => event._tag === "run_terminal")).toHaveLength(1);
		} finally {
			await rm(directory, { recursive: true, force: true });
		}
	});

	it("resumes with the supplied session identity", async () => {
		const engine = await get_engine();
		const native_thread_id = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const run = yield* engine.Open(open_input("resume"));
					return run.native_thread_id;
				}),
			),
		);
		expect(native_thread_id).toBe("resume-session");
	});

	it("does not fabricate an empty user turn when resume text is absent", async () => {
		const directory = await mkdtemp(join(process.cwd(), ".claude-resume-test-"));
		const invocation = join(directory, "invocations.jsonl");
		process.env.FAKE_CLAUDE_INVOCATION_FILE = invocation;

		try {
			const engine = await get_engine();

			await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const run = yield* engine.Open({
							_tag: "resume",
							artisan_run_id: "claude-empty-resume",
							working_directory: process.cwd(),
							resume_token: { native_thread_id: "resume-session" },
						});

						yield* run.Events.pipe(Stream.runDrain);
					}),
				),
			);

			const records = (await readFile(invocation, "utf8"))
				.trim()
				.split("\n")
				.map((line) => JSON.parse(line));
			const stdin = records.findLast((record) => record.stdin !== undefined)?.stdin;

			expect(stdin).toBe("");
		} finally {
			await rm(directory, { recursive: true, force: true });
		}
	});

	it("rejects permission/provider configuration before spawning", async () => {
		let spawn_count = 0;
		const factory = Layer.succeed(EngineProcessFactory, {
			Spawn: () => {
				spawn_count += 1;
				return Effect.die("spawned");
			},
		});
		const engine = await Effect.runPromise(
			ClaudeEngine.pipe(
				Effect.provide(
					make_claude_engine_layer({ executable }).pipe(Layer.provide(factory)),
				),
			),
		);
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.exit(
					engine.Open({
						...open_input(),
						permission_policy: {
							approval: "on_request",
							network_access: false,
							write_access: false,
						},
					}),
				),
			),
		);
		expect(Exit.isFailure(result)).toBe(true);
		expect(spawn_count).toBe(0);
	});

	it("fails semantically even when Claude exits zero, and retries unsupported commands", async () => {
		process.env.FAKE_CLAUDE_SCENARIO = "semantic-failure";
		const engine = await get_engine();
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const run = yield* engine.Open(open_input());
					const events = yield* run.Events.pipe(Stream.runCollect);
					const unsupported = yield* Effect.exit(
						run.Send({ _tag: "steer", command_id: "unsupported", text: "queued" }),
					);
					const retry = yield* Effect.exit(
						run.Send({ _tag: "steer", command_id: "unsupported", text: "queued" }),
					);
					return { events, retry, unsupported };
				}),
			),
		);
		const events = result.events;
		expect(events.at(-1)).toMatchObject({ _tag: "run_terminal", state: "failed" });
		const { unsupported, retry } = result;
		expect(Exit.isFailure(unsupported)).toBe(true);
		expect(Exit.isFailure(retry)).toBe(true);
	});

	it("supports cancel and close with exact-one terminals", async () => {
		process.env.FAKE_CLAUDE_SCENARIO = "timeout";
		const engine = await get_engine({ timeout_ms: 250 });
		const events = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const run = yield* engine.Open(open_input());
					yield* run.Send({ _tag: "cancel", command_id: "cancel" });
					return yield* run.Events.pipe(Stream.runCollect);
				}),
			),
		);
		expect(events.filter((event) => event._tag === "run_terminal")).toHaveLength(1);
	});

	it.each(["missing-init", "mismatch", "nonzero", "malformed"] as const)(
		"fails deterministically for %s",
		async (scenario) => {
			process.env.FAKE_CLAUDE_SCENARIO = scenario;
			const engine = await get_engine();
			const events = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const run = yield* engine.Open(open_input());
						return yield* run.Events.pipe(Stream.runCollect);
					}),
				),
			);
			expect(events.at(-1)).toMatchObject({
				_tag: "run_terminal",
				state: scenario === "malformed" ? "completed" : "failed",
			});
			expect(events.filter((event) => event._tag === "run_terminal")).toHaveLength(1);
		},
	);

	it("recovers malformed/oversized frames, drains stderr, and keeps terminal last", async () => {
		process.env.FAKE_CLAUDE_SCENARIO = "oversized";
		const engine = await get_engine({ max_frame_bytes: 128 });
		const events = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const run = yield* engine.Open(open_input());
					return yield* run.Events.pipe(Stream.runCollect);
				}),
			),
		);
		expect(events.some((event) => event._tag === "process_diagnostic")).toBe(true);
		expect(events.at(-1)?._tag).toBe("run_terminal");
		process.env.FAKE_CLAUDE_SCENARIO = "stderr";
		const stderr_engine = await get_engine();
		const stderr_events = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const run = yield* stderr_engine.Open(open_input());
					return yield* run.Events.pipe(Stream.runCollect);
				}),
			),
		);
		expect(stderr_events.some((event) => event._tag === "process_diagnostic")).toBe(true);
	});

	it("fails bounded backpressure with one terminal", async () => {
		process.env.FAKE_CLAUDE_SCENARIO = "flood";
		const engine = await get_engine({ event_capacity: 2 });
		const events = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const run = yield* engine.Open(open_input());
					return yield* run.Events.pipe(Stream.runCollect);
				}),
			),
		);
		expect(events.at(-1)?._tag).toBe("run_terminal");
		expect(events.filter((event) => event._tag === "run_terminal")).toHaveLength(1);
	});

	it("cleans a Claude process tree after terminal exit", async () => {
		const directory = await mkdtemp(join(process.cwd(), ".claude-tree-"));
		const pid_file = join(directory, "grandchild.pid");
		process.env.FAKE_CLAUDE_GRANDCHILD_PID_FILE = pid_file;
		try {
			const engine = await get_engine();
			await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const run = yield* engine.Open(open_input());
						yield* run.Events.pipe(Stream.runCollect);
					}),
				),
			);
			const pid = Number(await readFile(pid_file, "utf8"));
			for (let attempt = 0; attempt < 50; attempt += 1) {
				try {
					process.kill(pid, 0);
					await new Promise((resolve) => setTimeout(resolve, 10));
				} catch {
					return;
				}
			}
			throw new Error("Claude grandchild survived cleanup");
		} finally {
			delete process.env.FAKE_CLAUDE_GRANDCHILD_PID_FILE;
			await rm(directory, { recursive: true, force: true });
		}
	});

	const live_probe = process.env.ARTISAN_ENGINE_LIVE === "1" ? it : it.skip;
	live_probe("runs only the non-billable live version/auth probe", async () => {
		const engine = await get_engine({ executable: "claude" });
		const probe = await Effect.runPromise(engine.Probe({ client_name: "artisan-tests" }));
		expect(probe.version).toMatch(/\d+\.\d+\.\d+/);
	});
});
