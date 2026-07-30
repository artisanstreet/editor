import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { make_backend_runtime } from "@artisan/backend";
import type { EngineObservation } from "@artisan/engines";
import type { EventEnvelope } from "@artisan/protocol";

import {
	ApplyEngineObservation,
	ApplyJournalEvent,
	ConversationReadModel,
} from "../../modules/backend/src/conversation";
import { Database } from "../../modules/backend/src/persistence/database";
import {
	JournalCommands,
	MessageImageAttachments,
	Threads,
} from "../../modules/backend/src/persistence/tables";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: string[] = [];

const MakePath = async () => {
	const directory = await mkdtemp(join(tmpdir(), "artisan-conversation-"));
	directories.push(directory);
	return join(directory, "artisan.db");
};

afterEach(async () => {
	await Promise.all(
		directories.splice(0).map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

const Delta = (observation_id: string, sequence: number, delta: string): EngineObservation =>
	({
		_tag: "agent_message_delta",
		artisan_run_id: "run_1",
		delta,
		item_id: "assistant_1",
		observation_id,
		phase: "unspecified",
		raw: { engine_id: "codex", frame: {}, transport: "test" },
		sequence,
		turn_id: "turn_1",
	}) as EngineObservation;

describe("conversation projection", () => {
	it("reads exact image bytes only for the attachment's owning thread", async () => {
		const runtime = make_backend_runtime({ database_path: await MakePath(), migrations_path });
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const read_model = yield* ConversationReadModel;
					const now = "2026-07-24T00:00:00.000Z";
					yield* database.client.insert(Threads).values([
						{
							created_at: now,
							last_activity_at: now,
							thread_id: "thread_owner",
							title: "Owner",
							updated_at: now,
						},
						{
							created_at: now,
							last_activity_at: now,
							thread_id: "thread_other",
							title: "Other",
							updated_at: now,
						},
					]);
					yield* database.client.insert(JournalCommands).values({
						accepted_at: now,
						assigned_run_id: null,
						agent_id: null,
						causation_id: null,
						message_id: "message_owner",
						origin: "frontend",
						payload_json: "{}",
						payload_type: "thread.send_message",
						raw_origin_json: null,
						run_id: null,
						schema_version: 1,
						sent_at: now,
						status: "accepted",
						thread_id: "thread_owner",
					});
					yield* database.client.insert(MessageImageAttachments).values({
						attachment_id: "attachment_owner",
						content: Buffer.from([137, 80, 78, 71]),
						media_type: "image/png",
						message_id: "message_owner",
						name: "diagram.png",
						position: 0,
						size_bytes: 4,
					});

					return yield* Effect.all([
						read_model.ReadImageAttachment({
							attachment_id: "attachment_owner",
							thread_id: "thread_owner",
						}),
						read_model.ReadImageAttachment({
							attachment_id: "attachment_owner",
							thread_id: "thread_other",
						}),
						read_model.ReadImageAttachment({
							attachment_id: "missing_attachment",
							thread_id: "thread_owner",
						}),
					]);
				}),
			);

			expect(result[0]).toMatchObject({
				_tag: "Some",
				value: {
					bytes: new Uint8Array([137, 80, 78, 71]),
					id: "attachment_owner",
					media_type: "image/png",
					name: "diagram.png",
					size_bytes: 4,
				},
			});
			expect(result[1]).toMatchObject({ _tag: "None" });
			expect(result[2]).toMatchObject({ _tag: "None" });
		} finally {
			await runtime.dispose();
		}
	});

	it("keeps provider identity stable across deltas, duplicates, and completion", async () => {
		const runtime = make_backend_runtime({ database_path: await MakePath(), migrations_path });
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const read_model = yield* ConversationReadModel;
					yield* database.client.insert(Threads).values({
						created_at: "2026-07-24T00:00:00.000Z",
						last_activity_at: "2026-07-24T00:00:00.000Z",
						thread_id: "thread_1",
						title: "Conversation",
						updated_at: "2026-07-24T00:00:00.000Z",
					});
					yield* database.client.transaction((transaction) =>
						Effect.gen(function* () {
							const context = {
								occurred_at: "2026-07-24T00:00:01.000Z",
								run_id: "run_1",
								thread_id: "thread_1",
							};
							yield* ApplyEngineObservation(
								transaction,
								Delta("observation_1", 1, "Hello "),
								context,
							) as Effect.Effect<unknown, unknown, never>;
							yield* ApplyEngineObservation(
								transaction,
								{
									_tag: "turn_state",
									artisan_run_id: "run_1",
									observation_id: "observation_4",
									raw: { engine_id: "codex", frame: {}, transport: "test" },
									sequence: 4,
									state: "completed",
									turn_id: "turn_1",
								},
								context,
							) as Effect.Effect<unknown, unknown, never>;
							yield* ApplyEngineObservation(
								transaction,
								{
									_tag: "file",
									action: "modified",
									artisan_run_id: "run_1",
									observation_id: "observation_5",
									path: "src/app.ts",
									raw: { engine_id: "codex", frame: {}, transport: "test" },
									sequence: 5,
								},
								context,
							) as Effect.Effect<unknown, unknown, never>;
							yield* ApplyEngineObservation(
								transaction,
								Delta("observation_1", 1, "Hello "),
								context,
							) as Effect.Effect<unknown, unknown, never>;
							yield* ApplyEngineObservation(
								transaction,
								Delta("observation_2", 2, "world"),
								context,
							) as Effect.Effect<unknown, unknown, never>;
							yield* ApplyEngineObservation(
								transaction,
								{
									_tag: "agent_message_completed",
									artisan_run_id: "run_1",
									item_id: "assistant_1",
									message: "Hello world",
									observation_id: "observation_3",
									phase: "final",
									raw: { engine_id: "codex", frame: {}, transport: "test" },
									sequence: 3,
									turn_id: "turn_1",
								},
								context,
							) as Effect.Effect<unknown, unknown, never>;
						}),
					);
					return {
						patches: yield* read_model.ReadPatches("thread_1", 0),
						snapshot: yield* read_model.ReadSnapshot("thread_1"),
					};
				}),
			);

			expect(result.snapshot.status).toBe("available");
			if (result.snapshot.status !== "available") return;
			expect(
				result.snapshot.snapshot.items.find((item) => item.id === "assistant_1"),
			).toMatchObject({
				id: "assistant_1",
				lifecycle: "completed",
				phase: "final",
				text: "Hello world",
				turn_id: "run:run_1",
				type: "assistant_message",
			});
			expect(result.snapshot.snapshot.turns).toEqual([
				expect.objectContaining({ id: "run:run_1", run_id: "run_1" }),
			]);
			expect(
				result.snapshot.snapshot.items.filter((item) => item.type === "work_session"),
			).toHaveLength(1);
			expect(result.snapshot.snapshot.items.map((item) => item.type)).toEqual(
				expect.arrayContaining(["work_session", "change_set", "file_change"]),
			);
			expect(result.patches.map((patch) => patch.type)).toContain("item_append");
			expect(result.patches.map((patch) => patch.sequence)).toEqual(
				result.patches.map((_, index) => index + 1),
			);
		} finally {
			await runtime.dispose();
		}
	});

	it("projects a queued user message once with its accepted command identity", async () => {
		const runtime = make_backend_runtime({ database_path: await MakePath(), migrations_path });
		try {
			const availability = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const read_model = yield* ConversationReadModel;
					yield* database.client.insert(Threads).values({
						created_at: "2026-07-24T00:00:00.000Z",
						last_activity_at: "2026-07-24T00:00:00.000Z",
						thread_id: "thread_1",
						title: "Conversation",
						updated_at: "2026-07-24T00:00:00.000Z",
					});
					const event = {
						causation_id: "command_1",
						correlation_id: "command_1",
						journal_sequence: 1,
						kind: "event",
						message_id: "event_1",
						origin: "backend",
						payload: {
							message_id: "command_1",
							reason: "no_active_run",
							text: "Ship it",
							type: "thread.message_queued",
							working_directory: "C:\\workspace",
						},
						protocol_version: 1,
						schema_version: 1,
						sent_at: "2026-07-24T00:00:01.000Z",
						sequence: 1,
						stream_id: "thread:thread_1",
						thread_id: "thread_1",
					} satisfies EventEnvelope;
					yield* database.client.transaction((transaction) =>
						Effect.gen(function* () {
							yield* ApplyJournalEvent(transaction, event) as Effect.Effect<
								unknown,
								unknown,
								never
							>;
							yield* ApplyJournalEvent(transaction, event) as Effect.Effect<
								unknown,
								unknown,
								never
							>;
						}),
					);
					return yield* read_model.ReadSnapshot("thread_1");
				}),
			);
			expect(availability.status).toBe("available");
			if (availability.status !== "available") return;
			expect(availability.snapshot.items).toHaveLength(1);
			expect(availability.snapshot.items[0]).toMatchObject({
				id: "message:command_1",
				source_refs: [
					{
						event_id: "event_1",
						journal_sequence: 1,
						reference: "command_1",
					},
				],
				text: "Ship it",
				type: "user_message",
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("projects run lifecycle immediately so queued work is visible before the engine opens", async () => {
		const runtime = make_backend_runtime({ database_path: await MakePath(), migrations_path });
		try {
			const availability = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const read_model = yield* ConversationReadModel;
					yield* database.client.insert(Threads).values({
						created_at: "2026-07-24T00:00:00.000Z",
						last_activity_at: "2026-07-24T00:00:00.000Z",
						thread_id: "thread_1",
						title: "Conversation",
						updated_at: "2026-07-24T00:00:00.000Z",
					});
					const make_event = (
						journal_sequence: number,
						message_id: string,
						state: "queued" | "running" | "failed",
					) =>
						({
							agent_id: "agent_1",
							causation_id: "command_1",
							correlation_id: "command_1",
							journal_sequence,
							kind: "event",
							message_id,
							origin: "backend",
							payload: {
								state,
								type: "run.lifecycle",
								working_directory: "C:\\workspace",
							},
							protocol_version: 1,
							run_id: "run_1",
							schema_version: 1,
							sent_at: `2026-07-24T00:00:0${journal_sequence}.000Z`,
							sequence: journal_sequence,
							stream_id: "thread:thread_1",
							thread_id: "thread_1",
						}) satisfies EventEnvelope;

					yield* database.client.transaction((transaction) =>
						Effect.gen(function* () {
							yield* ApplyJournalEvent(
								transaction,
								make_event(1, "event_queued", "queued"),
							) as Effect.Effect<unknown, unknown, never>;
							yield* ApplyJournalEvent(
								transaction,
								make_event(2, "event_running", "running"),
							) as Effect.Effect<unknown, unknown, never>;
							yield* ApplyJournalEvent(
								transaction,
								make_event(3, "event_failed", "failed"),
							) as Effect.Effect<unknown, unknown, never>;
						}),
					);
					return yield* read_model.ReadSnapshot("thread_1");
				}),
			);

			expect(availability.status).toBe("available");
			if (availability.status !== "available") return;
			expect(availability.snapshot.turns).toEqual([
				expect.objectContaining({
					id: "run:run_1",
					lifecycle: "failed",
					run_id: "run_1",
				}),
			]);
			expect(availability.snapshot.items).toEqual([
				expect.objectContaining({
					ended_at: "2026-07-24T00:00:03.000Z",
					id: "work:run:run_1",
					lifecycle: "failed",
					started_at: "2026-07-24T00:00:01.000Z",
					status: "failed",
					type: "work_session",
				}),
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("projects typed approval details without provider method or item identifiers", async () => {
		const runtime = make_backend_runtime({ database_path: await MakePath(), migrations_path });
		try {
			const availability = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const read_model = yield* ConversationReadModel;
					yield* database.client.insert(Threads).values({
						created_at: "2026-07-24T00:00:00.000Z",
						last_activity_at: "2026-07-24T00:00:00.000Z",
						thread_id: "thread_1",
						title: "Conversation",
						updated_at: "2026-07-24T00:00:00.000Z",
					});
					yield* database.client.transaction((transaction) =>
						Effect.gen(function* () {
							const context = {
								occurred_at: "2026-07-24T00:00:01.000Z",
								run_id: "run_1",
								thread_id: "thread_1",
							};
							yield* ApplyEngineObservation(
								transaction,
								{
									_tag: "approval",
									approval_id: "opaque_response_id",
									artisan_run_id: "run_1",
									description: "Run the test suite",
									observation_id: "approval_observation",
									raw: {
										engine_id: "codex",
										frame: {
											itemId: "call_command",
											method: "item/commandExecution/requestApproval",
										},
										transport: "test",
									},
									request: {
										command: "pnpm test",
										cwd: "C:\\workspace",
										kind: "command",
										reason: "Run the test suite",
									},
									sequence: 1,
									state: "requested",
								},
								context,
							) as Effect.Effect<unknown, unknown, never>;
							yield* ApplyEngineObservation(
								transaction,
								{
									_tag: "approval",
									approval_id: "blank_response_id",
									artisan_run_id: "run_1",
									description: "  ",
									observation_id: "blank_approval_observation",
									raw: { engine_id: "test", frame: {}, transport: "test" },
									request: {
										command: "\t",
										cwd: " ",
										kind: "command",
										reason: "\n",
									},
									sequence: 2,
									state: "requested",
								},
								context,
							) as Effect.Effect<unknown, unknown, never>;
						}),
					);
					return yield* read_model.ReadSnapshot("thread_1");
				}),
			);

			expect(availability.status).toBe("available");
			if (availability.status !== "available") return;
			const approval = availability.snapshot.items.find((item) => item.type === "approval");
			expect(approval).toMatchObject({
				interaction_id: "opaque_response_id",
				prompt: "Run the test suite",
				request: {
					command: "pnpm test",
					cwd: "C:\\workspace",
					kind: "command",
					reason: "Run the test suite",
				},
				type: "approval",
			});
			expect(JSON.stringify(approval)).not.toContain("item/commandExecution");
			expect(JSON.stringify(approval)).not.toContain("call_command");
			expect(
				availability.snapshot.items.find(
					(item) =>
						item.type === "approval" && item.interaction_id === "blank_response_id",
				),
			).toMatchObject({
				prompt: "Approval requested",
				request: { kind: "command" },
				type: "approval",
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("completes a reasoning summary streamed by delta and emits an item_lifecycle patch", async () => {
		const runtime = make_backend_runtime({ database_path: await MakePath(), migrations_path });
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const read_model = yield* ConversationReadModel;
					yield* database.client.insert(Threads).values({
						created_at: "2026-07-24T00:00:00.000Z",
						last_activity_at: "2026-07-24T00:00:00.000Z",
						thread_id: "thread_1",
						title: "Conversation",
						updated_at: "2026-07-24T00:00:00.000Z",
					});
					const context = {
						occurred_at: "2026-07-24T00:00:01.000Z",
						run_id: "run_1",
						thread_id: "thread_1",
					};
					yield* database.client.transaction((transaction) =>
						Effect.gen(function* () {
							yield* ApplyEngineObservation(
								transaction,
								{
									_tag: "reasoning_summary_delta",
									artisan_run_id: "run_1",
									delta: "Weighing the options",
									item_id: "reasoning_1",
									observation_id: "observation_reasoning_delta",
									raw: { engine_id: "codex", frame: {}, transport: "test" },
									sequence: 1,
									summary_index: 0,
									turn_id: "turn_1",
								},
								context,
							) as Effect.Effect<unknown, unknown, never>;
							yield* ApplyEngineObservation(
								transaction,
								{
									_tag: "reasoning_summary_completed",
									artisan_run_id: "run_1",
									item_id: "reasoning_1",
									observation_id: "observation_reasoning_completed",
									raw: { engine_id: "codex", frame: {}, transport: "test" },
									sequence: 2,
									turn_id: "turn_1",
								},
								context,
							) as Effect.Effect<unknown, unknown, never>;
						}),
					);
					return {
						patches: yield* read_model.ReadPatches("thread_1", 0),
						snapshot: yield* read_model.ReadSnapshot("thread_1"),
					};
				}),
			);

			expect(result.snapshot.status).toBe("available");
			if (result.snapshot.status !== "available") return;
			expect(
				result.snapshot.snapshot.items.find((item) => item.id === "reasoning_1"),
			).toMatchObject({
				id: "reasoning_1",
				lifecycle: "completed",
				text: "Weighing the options",
				type: "reasoning_summary",
			});
			expect(
				result.patches.some(
					(patch) =>
						patch.type === "item_lifecycle" &&
						patch.item_id === "reasoning_1" &&
						patch.lifecycle === "completed",
				),
			).toBe(true);
		} finally {
			await runtime.dispose();
		}
	});

	it("no-ops a reasoning summary completion when no delta ever created the item", async () => {
		const runtime = make_backend_runtime({ database_path: await MakePath(), migrations_path });
		try {
			const availability = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const read_model = yield* ConversationReadModel;
					yield* database.client.insert(Threads).values({
						created_at: "2026-07-24T00:00:00.000Z",
						last_activity_at: "2026-07-24T00:00:00.000Z",
						thread_id: "thread_1",
						title: "Conversation",
						updated_at: "2026-07-24T00:00:00.000Z",
					});
					const context = {
						occurred_at: "2026-07-24T00:00:01.000Z",
						run_id: "run_1",
						thread_id: "thread_1",
					};
					yield* database.client.transaction(
						(transaction) =>
							ApplyEngineObservation(
								transaction,
								{
									_tag: "reasoning_summary_completed",
									artisan_run_id: "run_1",
									item_id: "reasoning_never_streamed",
									observation_id: "observation_reasoning_completed_only",
									raw: { engine_id: "claude", frame: {}, transport: "test" },
									sequence: 1,
									turn_id: "turn_1",
								},
								context,
							) as Effect.Effect<unknown, unknown, never>,
					);
					return yield* read_model.ReadSnapshot("thread_1");
				}),
			);

			expect(availability.status).toBe("available");
			if (availability.status !== "available") return;
			expect(
				availability.snapshot.items.find((item) => item.id === "reasoning_never_streamed"),
			).toBeUndefined();
			expect(
				availability.snapshot.items.filter((item) => item.type === "reasoning_summary"),
			).toHaveLength(0);
		} finally {
			await runtime.dispose();
		}
	});

	it("summarizes a native event from detail, falling back to action, and caps its length", async () => {
		const runtime = make_backend_runtime({ database_path: await MakePath(), migrations_path });
		try {
			const overlong_detail = "x".repeat(5_000);
			const availability = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const read_model = yield* ConversationReadModel;
					yield* database.client.insert(Threads).values({
						created_at: "2026-07-24T00:00:00.000Z",
						last_activity_at: "2026-07-24T00:00:00.000Z",
						thread_id: "thread_1",
						title: "Conversation",
						updated_at: "2026-07-24T00:00:00.000Z",
					});
					const context = {
						occurred_at: "2026-07-24T00:00:01.000Z",
						run_id: "run_1",
						thread_id: "thread_1",
					};
					yield* database.client.transaction((transaction) =>
						Effect.gen(function* () {
							yield* ApplyEngineObservation(
								transaction,
								{
									_tag: "native_action",
									action: "claude_event",
									artisan_run_id: "run_1",
									detail: "Provider quota narrowed to safe mode",
									observation_id: "observation_native_detail",
									raw: { engine_id: "claude", frame: {}, transport: "test" },
									sequence: 1,
								},
								context,
							) as Effect.Effect<unknown, unknown, never>;
							yield* ApplyEngineObservation(
								transaction,
								{
									_tag: "native_action",
									action: "claude_event",
									artisan_run_id: "run_1",
									observation_id: "observation_native_no_detail",
									raw: { engine_id: "claude", frame: {}, transport: "test" },
									sequence: 2,
								},
								context,
							) as Effect.Effect<unknown, unknown, never>;
							yield* ApplyEngineObservation(
								transaction,
								{
									_tag: "native_action",
									action: "claude_event",
									artisan_run_id: "run_1",
									detail: overlong_detail,
									observation_id: "observation_native_overlong",
									raw: { engine_id: "claude", frame: {}, transport: "test" },
									sequence: 3,
								},
								context,
							) as Effect.Effect<unknown, unknown, never>;
						}),
					);
					return yield* read_model.ReadSnapshot("thread_1");
				}),
			);

			expect(availability.status).toBe("available");
			if (availability.status !== "available") return;
			const native_events = availability.snapshot.items.filter(
				(item) => item.type === "native_event",
			);
			expect(
				native_events.find((item) => item.id === "native:observation_native_detail"),
			).toMatchObject({ summary: "Provider quota narrowed to safe mode" });
			expect(
				native_events.find((item) => item.id === "native:observation_native_no_detail"),
			).toMatchObject({ summary: "claude_event" });
			const overlong_item = native_events.find(
				(item) => item.id === "native:observation_native_overlong",
			);
			expect(overlong_item).toBeDefined();
			if (overlong_item?.type === "native_event") {
				expect(overlong_item.summary.length).toBe(4_096);
				expect(overlong_item.summary).toBe(overlong_detail.slice(0, 4_096));
			}
		} finally {
			await runtime.dispose();
		}
	});
});
