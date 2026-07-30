import { createHash } from "node:crypto";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Option, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { AgentOrchestrator, make_backend_runtime, ThreadErasure } from "@artisan/backend";
import type {
	Engine,
	EngineCapabilities,
	EngineContinuationExportInput,
	EngineNativeContinuationInput,
	EngineObservation,
	EngineOpenInput,
	EngineRun,
} from "@artisan/engines";

import { Database } from "../../modules/backend/src/persistence/database";
import { JournalStore } from "../../modules/backend/src/persistence/journal-store";
import type { AuthoritativeCommandEnvelope } from "../../modules/backend/src/persistence/orchestration/message-command";
import {
	OrchestrationRuns,
	ThreadErasureClaims,
} from "../../modules/backend/src/persistence/schema";
import {
	ThreadContinuationLaunches,
	ThreadPortableHandoffs,
	ThreadRunContinuationState,
} from "../../modules/backend/src/persistence/thread-continuation-schema";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: Array<string> = [];
const thread_id = "thread_continuation";
const sent_at = "2026-07-30T10:00:00.000Z";

const make_database_path = async () => {
	const directory = await mkdtemp(join(tmpdir(), "artisan-continuation-orchestration-"));
	directories.push(directory);
	return join(directory, "artisan.db");
};

const command = <const Payload extends AuthoritativeCommandEnvelope["payload"]>(
	message_id: string,
	payload: Payload,
): Omit<AuthoritativeCommandEnvelope, "payload"> & { readonly payload: Payload } => ({
	kind: "command",
	message_id,
	origin: "frontend",
	payload,
	protocol_version: 1,
	schema_version: 1,
	sent_at,
	thread_id,
});

const capabilities = (engine_id: "claude" | "codex"): EngineCapabilities => ({
	approval: { state: "supported" },
	auth: { state: "supported" },
	cancel: { state: "supported" },
	close: { state: "supported" },
	continuation_export:
		engine_id === "codex"
			? { state: "supported" }
			: { reason: "Claude uses PostCompact capture", state: "unsupported" },
	events: { state: "supported" },
	global_guidance: { state: "supported" },
	model_selection: { state: "supported" },
	native_continuation: { state: "supported" },
	native_tools: { reason: "Deterministic test engine", state: "unsupported" },
	probe: { state: "supported" },
	question: { state: "supported" },
	raw_frames: { state: "supported" },
	resume: { state: "supported" },
	start: { state: "supported" },
	steer: { reason: "Every follow-up exercises continuation", state: "unsupported" },
	subagents: { reason: "Deterministic test engine", state: "unsupported" },
});

interface EngineInstrumentation {
	readonly exports: Array<EngineContinuationExportInput>;
	readonly native_checks: Array<EngineNativeContinuationInput>;
	readonly open_inputs: Array<EngineOpenInput>;
}

const make_engine = (
	engine_id: "claude" | "codex",
	options: {
		readonly event_delay_ms?: number;
		readonly open_order?: Array<string>;
	} = {},
): { readonly engine: Engine; readonly instrumentation: EngineInstrumentation } => {
	const instrumentation: EngineInstrumentation = {
		exports: [],
		native_checks: [],
		open_inputs: [],
	};

	const Open = (input: EngineOpenInput) =>
		Effect.sync(() => {
			instrumentation.open_inputs.push(input);
			options.open_order?.push(input.artisan_run_id);
			const native_thread_id =
				input._tag === "resume"
					? input.resume_token.native_thread_id
					: `${engine_id}-native:${input.artisan_run_id}`;
			const turn_id = `${engine_id}-turn:${input.artisan_run_id}`;
			const boundary_id = `claude-boundary:${input.artisan_run_id}`;
			const compaction_observation_id = `${input.artisan_run_id}:compaction`;
			const summary = "Claude compacted objective, durable decisions, and verification.";
			const observations: Array<EngineObservation> = [];
			let sequence = 0;
			if (engine_id === "claude") {
				observations.push({
					_tag: "compaction",
					artisan_run_id: input.artisan_run_id,
					observation_id: compaction_observation_id,
					raw: {
						engine_id,
						frame: {
							compactMetadata: { trigger: "auto" },
							subtype: "compact_boundary",
							type: "system",
							uuid: boundary_id,
						},
						native_id: boundary_id,
						native_method: "system.compact_boundary",
						transport: "test",
					},
					sequence: ++sequence,
					state: "completed",
				});
			}
			observations.push(
				{
					_tag: "agent_message_completed",
					artisan_run_id: input.artisan_run_id,
					item_id: `${engine_id}-item:${input.artisan_run_id}`,
					message:
						engine_id === "claude"
							? "Claude post-compaction answer."
							: "Codex settled answer.",
					observation_id: `${input.artisan_run_id}:message`,
					phase: "final",
					raw: { engine_id, frame: "message", transport: "test" },
					sequence: ++sequence,
					turn_id,
				},
				{
					_tag: "turn_state",
					artisan_run_id: input.artisan_run_id,
					observation_id: `${input.artisan_run_id}:turn`,
					raw: { engine_id, frame: "turn", transport: "test" },
					sequence: ++sequence,
					state: "completed",
					turn_id,
				},
				{
					_tag: "run_terminal",
					artisan_run_id: input.artisan_run_id,
					observation_id: `${input.artisan_run_id}:terminal`,
					raw: { engine_id, frame: "terminal", transport: "test" },
					sequence: ++sequence,
					state: "completed",
				},
			);
			const events =
				options.event_delay_ms === undefined
					? Stream.fromIterable(observations)
					: Stream.fromEffect(Effect.sleep(`${options.event_delay_ms} millis`)).pipe(
							Stream.flatMap(() => Stream.fromIterable(observations)),
						);
			return {
				artisan_run_id: input.artisan_run_id,
				Closed: Effect.succeed("completed" as const),
				Events: events,
				...(engine_id === "claude"
					? {
							NativeCompaction: Effect.succeed(
								Option.some({
									boundary_id,
									method: "claude_post_compact" as const,
									observation_id: compaction_observation_id,
									source_native_thread_id: native_thread_id,
									summary,
									summary_sha256: createHash("sha256")
										.update(summary)
										.digest("hex"),
									trigger: "auto" as const,
								}),
							),
						}
					: {}),
				native_thread_id,
				resume_token: { native_thread_id },
				Send: () => Effect.void,
			} satisfies EngineRun;
		});

	const engine = {
		CheckNativeContinuation: (input: EngineNativeContinuationInput) =>
			Effect.sync(() => {
				instrumentation.native_checks.push(input);
				return { state: "compatible" as const };
			}),
		Descriptor: {
			capabilities: capabilities(engine_id),
			display_name: `Deterministic ${engine_id}`,
			id: engine_id,
			transport: "test",
		},
		...(engine_id === "codex"
			? {
					ExportContinuation: (input: EngineContinuationExportInput) =>
						Effect.sync(() => {
							instrumentation.exports.push(input);
							return {
								export_native_item_id: `export-item:${input.artisan_run_id}`,
								export_native_thread_id: `export-thread:${input.artisan_run_id}`,
								export_native_turn_id: `export-turn:${input.artisan_run_id}`,
								message: JSON.stringify({
									summary:
										"Codex exported objective, durable decisions, and verification.",
								}),
								method: "codex_fork_summary" as const,
								source_native_thread_id: input.source_resume_token.native_thread_id,
								source_native_turn_id: input.settled_native_turn_id,
							};
						}),
				}
			: {}),
		Open,
		Probe: () => Effect.die("Probe is not used by continuation integration tests"),
	} satisfies Engine;

	return { engine, instrumentation };
};

const set_policy = (
	orchestrator: typeof AgentOrchestrator.Service,
	engine_id: "claude" | "codex",
	model: string,
	message_id: string,
) =>
	orchestrator.Handle(
		command(message_id, {
			policy: {
				engine_id,
				model,
				permission_mode: "on_request",
				reasoning_effort: "medium",
				sandbox_mode: "workspace_write",
				service_tier: "standard",
				strict_clarification: false,
				web_search_enabled: false,
			},
			type: "thread.session_policy.update",
		}),
	);

const send = (
	orchestrator: typeof AgentOrchestrator.Service,
	message_id: string,
	engine_id: "claude" | "codex",
	text: string,
	multimodal = false,
) =>
	orchestrator.Handle(
		command(message_id, {
			...(multimodal
				? {
						attachments: [
							{
								bytes: new Uint8Array([
									0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a,
								]),
								id: `${message_id}_image`,
								media_type: "image/png" as const,
								name: "diagram.png",
							},
						],
						content: [
							{ text: "Current text before image.", type: "text" as const },
							{
								attachment_id: `${message_id}_image`,
								type: "image" as const,
							},
							{ text: "Current text after image.", type: "text" as const },
						],
					}
				: {}),
			engine_id,
			text,
			type: "thread.send_message",
			working_directory: "C:/work",
		}),
	);

const wait_for = async (predicate: () => boolean | Promise<boolean>) => {
	const deadline = Date.now() + 5_000;
	while (!(await predicate())) {
		if (Date.now() >= deadline) throw new Error("Timed out waiting for continuation");
		await new Promise((resolve) => setTimeout(resolve, 5));
	}
};

const wait_for_run = (
	runtime: ReturnType<typeof make_backend_runtime>,
	database: typeof Database.Service,
	run_id: string,
) =>
	wait_for(async () => {
		const rows = await runtime.runPromise(database.client.select().from(OrchestrationRuns));
		return rows.find((run) => run.run_id === run_id)?.status === "completed";
	});

afterEach(async () => {
	await Promise.all(
		directories.splice(0).map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("thread continuation orchestration", () => {
	it("hands Claude to Codex, Codex to Claude, then natively resumes a Claude model change", async () => {
		const claude = make_engine("claude");
		const codex = make_engine("codex");
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			engines: [claude.engine, codex.engine],
			migrations_path,
		});
		try {
			const { database, erasure, journal, orchestrator } = await runtime.runPromise(
				Effect.gen(function* () {
					return {
						database: yield* Database,
						erasure: yield* ThreadErasure,
						journal: yield* JournalStore,
						orchestrator: yield* AgentOrchestrator,
					};
				}),
			);
			await runtime.runPromise(
				journal.AcceptThreadCreate(
					command("create", {
						title: "Portable continuation",
						type: "thread.create",
					}),
				),
			);

			await runtime.runPromise(
				set_policy(orchestrator, "claude", "claude-sonnet-5", "policy_1"),
			);
			const first = await runtime.runPromise(
				send(orchestrator, "message_1", "claude", "Establish the implementation state."),
			);
			await wait_for_run(runtime, database, first.run_id);

			await runtime.runPromise(set_policy(orchestrator, "codex", "gpt-5.6-sol", "policy_2"));
			const second = await runtime.runPromise(
				send(
					orchestrator,
					"message_2",
					"codex",
					"Continue on Codex with the attached evidence.",
					true,
				),
			);
			await wait_for_run(runtime, database, second.run_id);
			await wait_for(() => codex.instrumentation.open_inputs.length === 1);
			const codex_input = codex.instrumentation.open_inputs[0]!;
			expect(codex_input._tag).toBe("start");
			if (codex_input._tag !== "start") throw new Error("Expected portable Codex start");
			expect(codex_input.initial_content?.[0]).toMatchObject({
				type: "text",
				text: expect.stringContaining(
					"Claude compacted objective, durable decisions, and verification.",
				),
			});
			expect(codex_input.initial_content?.slice(1)).toEqual([
				{ text: "Current text before image.", type: "text" },
				{
					bytes: new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
					id: "message_2_image",
					media_type: "image/png",
					name: "diagram.png",
					type: "image",
				},
				{ text: "Current text after image.", type: "text" },
			]);
			expect(JSON.stringify(codex_input)).not.toContain("claude-native:");
			expect(JSON.stringify(codex_input)).not.toContain("claude-boundary:");

			await runtime.runPromise(
				set_policy(orchestrator, "claude", "claude-sonnet-5", "policy_3"),
			);
			const third = await runtime.runPromise(
				send(orchestrator, "message_3", "claude", "Continue on Claude."),
			);
			await wait_for_run(runtime, database, third.run_id);
			await wait_for(() => claude.instrumentation.open_inputs.length === 2);
			const portable_claude_input = claude.instrumentation.open_inputs[1]!;
			expect(portable_claude_input._tag).toBe("start");
			if (portable_claude_input._tag !== "start")
				throw new Error("Expected portable Claude start");
			expect(portable_claude_input.initial_text).toContain(
				"Codex exported objective, durable decisions, and verification.",
			);
			expect(portable_claude_input.initial_text).toContain("Continue on Claude.");
			expect(JSON.stringify(portable_claude_input)).not.toContain("codex-native:");
			expect(JSON.stringify(portable_claude_input)).not.toContain("export-thread:");
			expect(codex.instrumentation.exports).toHaveLength(1);

			const handoffs_before_native = await runtime.runPromise(
				database.client.select().from(ThreadPortableHandoffs),
			);
			expect(handoffs_before_native).toHaveLength(2);
			const lineages = handoffs_before_native.map((handoff) =>
				JSON.parse(handoff.provider_lineage_json),
			);
			expect(lineages).toEqual(
				expect.arrayContaining([
					expect.objectContaining({
						boundary_id: expect.stringContaining("claude-boundary:"),
						kind: "claude",
						source_native_thread_id: expect.stringContaining("claude-native:"),
					}),
					expect.objectContaining({
						export_native_thread_id: expect.stringContaining("export-thread:"),
						kind: "codex",
						source_native_thread_id: expect.stringContaining("codex-native:"),
					}),
				]),
			);

			await runtime.runPromise(
				set_policy(orchestrator, "claude", "claude-opus-5", "policy_4"),
			);
			const fourth = await runtime.runPromise(
				send(
					orchestrator,
					"message_4",
					"claude",
					"Continue natively on the new Claude model.",
					true,
				),
			);
			await wait_for_run(runtime, database, fourth.run_id);
			await wait_for(() => claude.instrumentation.open_inputs.length === 3);
			const native_input = claude.instrumentation.open_inputs[2]!;
			expect(native_input).toMatchObject({
				_tag: "resume",
				model: "claude-opus-5",
				next_content: [
					{ text: "Current text before image.", type: "text" },
					expect.objectContaining({
						id: "message_4_image",
						type: "image",
					}),
					{ text: "Current text after image.", type: "text" },
				],
				next_text: "Continue natively on the new Claude model.",
			});
			expect(claude.instrumentation.native_checks.at(-1)).toMatchObject({
				source_model: "claude-sonnet-5",
				target_model: "claude-opus-5",
			});
			expect(
				await runtime.runPromise(database.client.select().from(ThreadPortableHandoffs)),
			).toHaveLength(2);

			await wait_for(async () => {
				const states = await runtime.runPromise(
					database.client.select().from(ThreadRunContinuationState),
				);
				return states.some(
					(state) =>
						state.run_id === fourth.run_id && state.native_compaction_json !== null,
				);
			});
			await runtime.runPromise(
				database.client
					.insert(ThreadErasureClaims)
					.values({ claimed_at: sent_at, thread_id }),
			);
			expect(await runtime.runPromise(erasure.ResumeClaimed(sent_at))).toEqual([thread_id]);
			expect(
				await runtime.runPromise(database.client.select().from(ThreadContinuationLaunches)),
			).toEqual([]);
			expect(
				await runtime.runPromise(database.client.select().from(ThreadPortableHandoffs)),
			).toEqual([]);
			expect(
				await runtime.runPromise(database.client.select().from(ThreadRunContinuationState)),
			).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});

	it("serializes three neighboring launches by their immediate journal predecessor", async () => {
		const open_order: Array<string> = [];
		const codex = make_engine("codex", { event_delay_ms: 80, open_order });
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			engines: [codex.engine],
			migrations_path,
		});
		try {
			const { database, journal, orchestrator } = await runtime.runPromise(
				Effect.gen(function* () {
					return {
						database: yield* Database,
						journal: yield* JournalStore,
						orchestrator: yield* AgentOrchestrator,
					};
				}),
			);
			await runtime.runPromise(
				journal.AcceptThreadCreate(
					command("create", { title: "Serialized", type: "thread.create" }),
				),
			);
			const first = await runtime.runPromise(
				send(orchestrator, "serial_1", "codex", "First"),
			);
			const second = await runtime.runPromise(
				send(orchestrator, "serial_2", "codex", "Second"),
			);
			const third = await runtime.runPromise(
				send(orchestrator, "serial_3", "codex", "Third"),
			);
			await wait_for(() => open_order.length === 1);
			expect(open_order).toEqual([first.run_id]);

			await Promise.all([
				wait_for_run(runtime, database, first.run_id),
				wait_for_run(runtime, database, second.run_id),
				wait_for_run(runtime, database, third.run_id),
			]);
			expect(open_order).toEqual([first.run_id, second.run_id, third.run_id]);
		} finally {
			await runtime.dispose();
		}
	});
});
