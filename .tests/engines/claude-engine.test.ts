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

const executable = fileURLToPath(new URL("./fixtures/fake-claude.ts", import.meta.url));
const original_scenario = process.env.FAKE_CLAUDE_SCENARIO;
const original_invocation = process.env.FAKE_CLAUDE_INVOCATION_FILE;
const original_compaction_directory = process.env.FAKE_CLAUDE_COMPACTION_DIRECTORY;
const original_version = process.env.FAKE_CLAUDE_VERSION;

afterEach(() => {
	if (original_scenario === undefined) delete process.env.FAKE_CLAUDE_SCENARIO;
	else process.env.FAKE_CLAUDE_SCENARIO = original_scenario;
	if (original_invocation === undefined) delete process.env.FAKE_CLAUDE_INVOCATION_FILE;
	else process.env.FAKE_CLAUDE_INVOCATION_FILE = original_invocation;
	if (original_compaction_directory === undefined)
		delete process.env.FAKE_CLAUDE_COMPACTION_DIRECTORY;
	else process.env.FAKE_CLAUDE_COMPACTION_DIRECTORY = original_compaction_directory;
	if (original_version === undefined) delete process.env.FAKE_CLAUDE_VERSION;
	else process.env.FAKE_CLAUDE_VERSION = original_version;
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
	it("declares truthful capabilities", () => {
		expect(ClaudeEngineDescriptor.capabilities.steer.state).toBe("unsupported");
		expect(ClaudeEngineDescriptor.capabilities.approval.state).toBe("unsupported");
		expect(ClaudeEngineDescriptor.capabilities.global_guidance.state).toBe("unsupported");
		expect(ClaudeEngineDescriptor.capabilities.model_selection.state).toBe("supported");
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

	it("version-tests native continuation and permits target-model changes only on 2.1.220", async () => {
		const engine = await get_engine();
		const exact = await Effect.runPromise(
			engine.CheckNativeContinuation!({
				resume_token: { native_thread_id: "resume-session" },
				source_model: "claude-sonnet-4",
				target_model: "claude-opus-4",
			}),
		);
		expect(exact).toEqual({ state: "compatible" });
		const missing_target = await Effect.runPromise(
			engine.CheckNativeContinuation!({
				resume_token: { native_thread_id: "resume-session" },
			}),
		);
		expect(missing_target).toMatchObject({ state: "incompatible" });
		process.env.FAKE_CLAUDE_VERSION = "2.1.219";
		const old_engine = await get_engine();
		const old = await Effect.runPromise(
			old_engine.CheckNativeContinuation!({
				resume_token: { native_thread_id: "resume-session" },
			}),
		);
		expect(old).toMatchObject({ state: "unsupported" });
		process.env.FAKE_CLAUDE_VERSION = "2.1.220-beta.1";
		const prerelease_engine = await get_engine();
		const prerelease = await Effect.runPromise(
			prerelease_engine.CheckNativeContinuation!({
				resume_token: { native_thread_id: "resume-session" },
				target_model: "claude-opus-4",
			}),
		);
		expect(prerelease).toMatchObject({ state: "unsupported" });
		process.env.FAKE_CLAUDE_VERSION = "3.0.0";
		const new_engine = await get_engine();
		const newer = await Effect.runPromise(
			new_engine.CheckNativeContinuation!({
				resume_token: { native_thread_id: "resume-session" },
			}),
		);
		expect(newer).toMatchObject({ state: "unsupported" });
		process.env.FAKE_CLAUDE_VERSION = "not-a-version";
		const invalid_engine = await get_engine();
		const invalid = await Effect.runPromise(
			Effect.exit(
				invalid_engine.CheckNativeContinuation!({
					resume_token: { native_thread_id: "resume-session" },
				}),
			),
		);
		expect(Exit.isFailure(invalid)).toBe(true);
	});

	it("emits a summary-free compaction observation before terminal", async () => {
		const directory = await mkdtemp(join(process.cwd(), ".claude-compact-test-"));
		process.env.FAKE_CLAUDE_SCENARIO = "post-compact";
		process.env.FAKE_CLAUDE_COMPACTION_DIRECTORY = directory;
		try {
			const engine = await get_engine({ claude_config_dir: directory });
			const result = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const run = yield* engine.Open(open_input());
						const events = yield* run.Events.pipe(Stream.runCollect);
						return { events };
					}),
				),
			);
			const compaction = result.events.find((event) => event._tag === "compaction");
			expect(compaction).toMatchObject({
				raw: {
					frame: {
						compactMetadata: { trigger: "auto" },
						subtype: "compact_boundary",
						type: "system",
						uuid: "boundary-1",
					},
					native_id: "boundary-1",
					native_method: "system.compact_boundary",
				},
			});
			expect(compaction?.observation_id).toMatch(
				/^claude-run:claude:\d+:compact_boundary:sequence:\d+$/,
			);
			expect(JSON.stringify(compaction)).not.toContain("captured compact summary");
			expect(result.events.indexOf(compaction!)).toBeLessThan(
				result.events.findIndex((event) => event._tag === "run_terminal"),
			);
		} finally {
			await rm(directory, { recursive: true, force: true });
		}
	});

	it("never adds a plugin directory to the spawned argv", async () => {
		const directory = await mkdtemp(join(process.cwd(), ".claude-plugin-free-test-"));
		const invocation = join(directory, "invocations.jsonl");
		process.env.FAKE_CLAUDE_INVOCATION_FILE = invocation;

		try {
			const engine = await get_engine({ claude_config_dir: directory });
			await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const run = yield* engine.Open(open_input());
						yield* run.Events.pipe(Stream.runCollect);
					}),
				),
			);
			const records = (await readFile(invocation, "utf8"))
				.trim()
				.split("\n")
				.map((line) => JSON.parse(line));
			const run_args = records.findLast((record) => record.args)?.args as string[];

			expect(run_args).not.toContain("--plugin-dir");
		} finally {
			await rm(directory, { recursive: true, force: true });
		}
	});

	it("enforces a provider-native no-tools boundary for constrained turns", async () => {
		const directory = await mkdtemp(join(process.cwd(), ".claude-no-tools-test-"));
		const invocation = join(directory, "invocations.jsonl");
		process.env.FAKE_CLAUDE_INVOCATION_FILE = invocation;

		try {
			const engine = await get_engine();
			await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const run = yield* engine.Open({
							...open_input(),
							provider_options: {
								"claude.disable_tools": true,
								"claude.permission_mode": "plan",
								"claude.safe_mode": true,
							},
						});
						yield* run.Events.pipe(Stream.runCollect);
					}),
				),
			);
			const records = (await readFile(invocation, "utf8"))
				.trim()
				.split("\n")
				.map((line) => JSON.parse(line) as { readonly args?: ReadonlyArray<string> });
			const args = records.findLast((record) => record.args)?.args ?? [];
			const tools_index = args.indexOf("--tools");

			expect(args).toContain("--safe-mode");
			expect(args).toContain("--permission-mode");
			expect(args[args.indexOf("--permission-mode") + 1]).toBe("plan");
			expect(tools_index).toBeGreaterThanOrEqual(0);
			expect(args[tools_index + 1]).toBe("");
		} finally {
			await rm(directory, { recursive: true, force: true });
		}
	});

	it("preserves ordered text/image parts as native stream-json content blocks", async () => {
		const directory = await mkdtemp(join(process.cwd(), ".claude-image-test-"));
		const invocation = join(directory, "invocations.jsonl");
		process.env.FAKE_CLAUDE_INVOCATION_FILE = invocation;

		try {
			const engine = await get_engine();

			await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const run = yield* engine.Open({
							_tag: "start",
							artisan_run_id: "claude-image-run",
							initial_text: "hello",
							working_directory: process.cwd(),
							initial_content: [
								{ text: "hello", type: "text" },
								{
									bytes: new Uint8Array([1, 2, 3]),
									id: "image-1",
									media_type: "image/png",
									name: "shot.png",
									type: "image",
								},
							],
						});
						yield* run.Events.pipe(Stream.runDrain);
					}),
				),
			);

			const records = (await readFile(invocation, "utf8"))
				.trim()
				.split("\n")
				.map((line) => JSON.parse(line));
			const stdin = records.findLast((record) => record.stdin !== undefined)?.stdin as string;

			expect(JSON.parse(stdin).message.content).toEqual([
				{ text: "hello", type: "text" },
				{
					source: { data: "AQID", media_type: "image/png", type: "base64" },
					type: "image",
				},
			]);
		} finally {
			await rm(directory, { recursive: true, force: true });
		}
	});

	it("sends content-only resume input without requiring next_text", async () => {
		const directory = await mkdtemp(join(process.cwd(), ".claude-content-resume-test-"));
		const invocation = join(directory, "invocations.jsonl");
		process.env.FAKE_CLAUDE_INVOCATION_FILE = invocation;

		try {
			const engine = await get_engine();
			await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const run = yield* engine.Open({
							_tag: "resume",
							artisan_run_id: "claude-content-resume",
							next_content: [
								{
									bytes: new Uint8Array([1, 2, 3]),
									id: "image-1",
									media_type: "image/png",
									name: "shot.png",
									type: "image",
								},
							],
							resume_token: { native_thread_id: "resume-session" },
							working_directory: process.cwd(),
						});
						yield* run.Events.pipe(Stream.runDrain);
					}),
				),
			);

			const records = (await readFile(invocation, "utf8"))
				.trim()
				.split("\n")
				.map((line) => JSON.parse(line));
			const stdin = records.findLast((record) => record.stdin !== undefined)?.stdin as string;
			expect(JSON.parse(stdin).message.content).toEqual([
				{
					source: { data: "AQID", media_type: "image/png", type: "base64" },
					type: "image",
				},
			]);
		} finally {
			await rm(directory, { recursive: true, force: true });
		}
	});

	it("maps the provider-owned prompt-file option to one native flag on start and resume", async () => {
		const directory = await mkdtemp(join(process.cwd(), ".claude-prompt-file-test-"));
		const invocation = join(directory, "invocations.jsonl");
		const prompt_file = "C:\\workspace\\CLAUDE.md";

		process.env.FAKE_CLAUDE_INVOCATION_FILE = invocation;

		try {
			const engine = await get_engine();

			await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const started = yield* engine.Open({
							...open_input(),
							provider_options: {
								"claude.append_system_prompt_file": prompt_file,
							},
						});
						yield* started.Events.pipe(Stream.runDrain);
						const resumed = yield* engine.Open({
							...open_input("resume"),
							provider_options: {
								"claude.append_system_prompt_file": prompt_file,
							},
						});
						yield* resumed.Events.pipe(Stream.runDrain);
					}),
				),
			);

			const records = (await readFile(invocation, "utf8"))
				.trim()
				.split("\n")
				.map((line) => JSON.parse(line));
			const run_args = records.filter((record) => record.args?.includes("-p") === true);
			const run_stdin = records.filter((record) => record.stdin !== undefined);

			expect(run_args).toHaveLength(2);
			expect(run_stdin).toHaveLength(2);
			expect(run_args.map((record) => record.args)).toEqual(
				expect.arrayContaining([
					expect.arrayContaining(["--append-system-prompt-file", prompt_file]),
					expect.arrayContaining(["--append-system-prompt-file", prompt_file]),
				]),
			);
			for (const record of run_args) {
				expect(
					(record.args as string[]).filter(
						(arg) => arg === "--append-system-prompt-file",
					),
				).toHaveLength(1);
			}
			expect(JSON.parse(run_stdin[0].stdin).message.content).toBe("hello");
			expect(JSON.parse(run_stdin[1].stdin).message.content).toBe("continue");
		} finally {
			await rm(directory, { recursive: true, force: true });
		}
	});

	it("rejects unsupported global guidance before spawning", async () => {
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
						global_guidance: {
							content: "Guidance",
							source_file: "C:\\workspace\\AGENTS.md",
						},
					}),
				),
			),
		);

		expect(Exit.isFailure(result)).toBe(true);
		expect(JSON.stringify(result)).toContain("EngineUnsupportedOperationError");
		expect(spawn_count).toBe(0);
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
	live_probe(
		"runs only the non-billable live version/auth probe",
		async () => {
			const engine = await get_engine({
				executable: "claude",
				version_timeout_ms: 30_000,
				auth_timeout_ms: 30_000,
			});
			const probe = await Effect.runPromise(engine.Probe({ client_name: "artisan-tests" }));
			expect(probe.version).toMatch(/\d+\.\d+\.\d+/);
		},
		70_000,
	);
});
