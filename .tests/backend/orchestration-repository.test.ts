import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { EngineObservation } from "@artisan/engines";
import type { CommandEnvelope } from "@artisan/protocol";
import type { AuthoritativeCommandEnvelope } from "../../modules/backend/src/persistence/orchestration/message-command";
import { make_backend_runtime, ThreadErasure } from "@artisan/backend";

import { ConversationReadModel } from "../../modules/backend/src/conversation";
import { OrchestrationRepository } from "../../modules/backend/src/persistence/orchestration/repository";
import type { IntakeAssessment } from "../../modules/backend/src/orchestration/intake-policy";
import {
	ConversationItems,
	JournalCommands,
	JournalEvents,
	MessageImageAttachments,
	OrchestrationMessages,
	OrchestrationOutbox,
	OrchestrationIntake,
	OrchestrationInteractions,
	OrchestrationRawObservations,
	OrchestrationRuns,
	Projects,
	SurfaceItems,
	Threads,
} from "../../modules/backend/src/persistence/tables";
import { Database } from "../../modules/backend/src/persistence/database";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-orchestration-repository-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

function make_command(
	message_id: string,
	thread_id: string,
	payload: AuthoritativeCommandEnvelope["payload"],
	options: Partial<Pick<AuthoritativeCommandEnvelope, "raw_origin" | "run_id">> = {},
): AuthoritativeCommandEnvelope {
	return {
		kind: "command",
		message_id,
		origin: "frontend",
		payload,
		protocol_version: 1,
		...(options.raw_origin ? { raw_origin: options.raw_origin } : {}),
		...(options.run_id ? { run_id: options.run_id } : {}),
		schema_version: 1,
		sent_at: "2026-07-10T10:00:00.000Z",
		thread_id,
	};
}

const SetupThread = (thread_id: string) =>
	Effect.gen(function* () {
		const database = yield* Database;

		yield* database.client.insert(Threads).values({
			created_at: "2026-07-10T10:00:00.000Z",
			thread_id,
			title: thread_id,
			updated_at: "2026-07-10T10:00:00.000Z",
		});
	});

const SetupFreshThread = (thread_id: string) =>
	Effect.gen(function* () {
		const database = yield* Database;

		yield* database.client.insert(Threads).values({
			created_at: "2099-01-01T00:00:00.000Z",
			last_activity_at: "2099-01-01T00:00:00.000Z",
			thread_id,
			title: thread_id,
			updated_at: "2099-01-01T00:00:00.000Z",
		});
	});

const Accept = (command: AuthoritativeCommandEnvelope, intake?: IntakeAssessment) =>
	Effect.gen(function* () {
		const repository = yield* OrchestrationRepository;

		return yield* repository.Accept(command, false, intake);
	});

const AcceptInbound = (command: CommandEnvelope) =>
	Effect.gen(function* () {
		const repository = yield* OrchestrationRepository;

		return yield* repository.AcceptInbound(command, false);
	});

const Read = <A>(
	read: (database: (typeof Database.Service)["client"]) => Effect.Effect<A, unknown>,
) =>
	Effect.gen(function* () {
		const database = yield* Database;

		return yield* read(database.client).pipe(Effect.orDie);
	});

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("orchestration repository hardening", () => {
	it("accepts an ordinary text message with an empty attachment list", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			migrations_path,
		});
		try {
			await runtime.runPromise(SetupThread("thread_1"));
			const accepted = await runtime.runPromise(
				Accept(
					make_command("message_1", "thread_1", {
						attachments: [],
						engine_id: "engine_1",
						text: "Hello",
						type: "thread.send_message",
						working_directory: "C:/work",
					}),
				),
			);
			const attachments = await runtime.runPromise(
				Read((database) => database.select().from(MessageImageAttachments)),
			);
			const [thread] = await runtime.runPromise(
				Read((database) => database.select().from(Threads)),
			);

			expect(accepted.status).toBe("accepted");
			expect(attachments).toEqual([]);
			expect(thread).toMatchObject({ live_status: "Working" });
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects a distinct message while the coordinator root is still queued", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			migrations_path,
		});
		try {
			await runtime.runPromise(SetupThread("thread_1"));
			const first = make_command("message_1", "thread_1", {
				engine_id: "engine_1",
				text: "First request",
				type: "thread.send_message",
				working_directory: "C:/work",
			});
			const accepted = await runtime.runPromise(Accept(first));
			const rejected = await runtime.runPromiseExit(
				Accept(
					make_command("message_2", "thread_1", {
						engine_id: "engine_1",
						text: "Second request",
						type: "thread.send_message",
						working_directory: "C:/work",
					}),
				),
			);
			const persisted = await runtime.runPromise(
				Read((database) =>
					Effect.all({
						events: database.select().from(JournalEvents),
						messages: database.select().from(OrchestrationMessages),
						outbox: database.select().from(OrchestrationOutbox),
						runs: database.select().from(OrchestrationRuns),
					}),
				),
			);

			expect(accepted.status).toBe("accepted");
			expect(rejected._tag).toBe("Failure");
			expect(persisted.runs).toMatchObject([{ run_id: accepted.run_id, status: "queued" }]);
			expect(persisted.outbox).toHaveLength(1);
			expect(persisted.messages).toHaveLength(1);
			expect(
				persisted.events.filter(
					(event) => JSON.parse(event.payload_json).type === "thread.message_queued",
				),
			).toHaveLength(1);
		} finally {
			await runtime.dispose();
		}
	});

	it("derives project authority and replaces client attachment tokens atomically", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			migrations_path,
		});
		const client_token = "temporary-client-token";
		try {
			await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					yield* database.client.insert(Projects).values({
						attached_at: "2026-07-10T10:00:00.000Z",
						display_name: "Workspace",
						project_id: "project_1",
						root_path: "C:/forge/workspace",
						updated_at: "2026-07-10T10:00:00.000Z",
					});
					yield* database.client.insert(Threads).values({
						created_at: "2026-07-10T10:00:00.000Z",
						primary_project_id: "project_1",
						thread_id: "thread_1",
						title: "thread_1",
						updated_at: "2026-07-10T10:00:00.000Z",
					});
				}),
			);

			const accepted = await runtime.runPromise(
				AcceptInbound({
					kind: "command",
					message_id: "message_1",
					origin: "frontend",
					payload: {
						attachments: [
							{
								bytes: new Uint8Array([
									0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a,
								]),
								client_token,
								media_type: "image/png",
								name: "image.png",
							},
						],
						content: [{ client_token, type: "image" }],
						engine_id: "engine_1",
						text: "Inspect this image",
						type: "thread.send_message",
					},
					protocol_version: 1,
					schema_version: 1,
					sent_at: "2026-07-10T10:00:00.000Z",
					thread_id: "thread_1",
				}),
			);
			const persisted = await runtime.runPromise(
				Read((database) =>
					Effect.all({
						attachments: database.select().from(MessageImageAttachments),
						commands: database.select().from(JournalCommands),
						outbox: database.select().from(OrchestrationOutbox),
					}),
				),
			);
			const durable_id = persisted.attachments[0]?.attachment_id;
			const queued = accepted.events.find(
				(event) => event.payload.type === "thread.message_queued",
			);

			expect(durable_id).toBeDefined();
			expect(durable_id).not.toBe(client_token);
			expect(JSON.stringify(persisted)).not.toContain(client_token);
			expect(queued?.payload).toMatchObject({
				content: [{ attachment_id: durable_id, type: "image" }],
				mentioned_projects: [
					{
						display_name: "Workspace",
						project_id: "project_1",
						root_path: "C:/forge/workspace",
					},
				],
				working_directory: "C:/forge/workspace",
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("projects an agent message delta without retaining its raw frame", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			migrations_path,
		});
		try {
			await runtime.runPromise(SetupThread("thread_1"));
			const accepted = await runtime.runPromise(
				Accept(
					make_command("send_1", "thread_1", {
						engine_id: "engine_1",
						text: "Start",
						type: "thread.send_message",
						working_directory: "C:/work",
					}),
				),
			);
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* OrchestrationRepository;

					return yield* repository.RecordObservation({
						_tag: "agent_message_delta",
						artisan_run_id: accepted.run_id,
						delta: "Partial response",
						item_id: "assistant_1",
						observation_id: "delta_1",
						phase: "unspecified",
						raw: {
							engine_id: "engine_1",
							frame: { text: "Partial response" },
							transport: "fixture",
						},
						sequence: 1,
						turn_id: "turn_1",
					});
				}),
			);
			const [raw, surfaces] = await runtime.runPromise(
				Effect.all([
					Read((database) => database.select().from(OrchestrationRawObservations)),
					Read((database) => database.select().from(SurfaceItems)),
				]),
			);

			expect(result).toEqual([]);
			expect(raw).toEqual([]);
			expect(surfaces).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});

	it("persists a streamed delta batch without duplicating provider frames", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			migrations_path,
		});
		try {
			await runtime.runPromise(SetupThread("thread_1"));
			const accepted = await runtime.runPromise(
				Accept(
					make_command("send_1", "thread_1", {
						engine_id: "engine_1",
						text: "Start",
						type: "thread.send_message",
						working_directory: "C:/work",
					}),
				),
			);
			const delta = (observation_id: string, sequence: number, text: string) =>
				({
					_tag: "agent_message_delta",
					artisan_run_id: accepted.run_id,
					delta: text,
					item_id: "assistant_1",
					observation_id,
					phase: "unspecified",
					raw: {
						engine_id: "engine_1",
						frame: { text },
						transport: "fixture",
					},
					sequence,
					turn_id: "turn_1",
				}) satisfies EngineObservation;
			const events = await runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* OrchestrationRepository;

					return yield* repository.RecordObservations([
						delta("delta_1", 1, "Hello"),
						delta("delta_2", 2, ", "),
						delta("delta_3", 3, "world"),
						{
							_tag: "agent_message_completed",
							artisan_run_id: accepted.run_id,
							item_id: "assistant_1",
							message: "Hello, world",
							observation_id: "completed_1",
							phase: "final",
							raw: {
								engine_id: "engine_1",
								frame: { text: "Hello, world" },
								transport: "fixture",
							},
							sequence: 4,
							turn_id: "turn_1",
						},
					]);
				}),
			);
			const [raw, items] = await runtime.runPromise(
				Effect.all([
					Read((database) => database.select().from(OrchestrationRawObservations)),
					Read((database) => database.select().from(ConversationItems)),
				]),
			);
			const assistant_item = items.find((item) => item.item_id === "assistant_1");
			const assistant_entity = JSON.parse(assistant_item?.entity_json ?? "{}");

			expect(events).toMatchObject([
				{ payload: { text: "Hello, world", type: "assistant.message_completed" } },
			]);
			expect(raw).toEqual([]);
			expect(assistant_entity).toMatchObject({
				lifecycle: "completed",
				text: "Hello, world",
				type: "assistant_message",
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("persists a pre-execution intake question without opening a run outbox", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			migrations_path,
		});
		try {
			await runtime.runPromise(SetupThread("thread_1"));
			const accepted = await runtime.runPromise(
				Accept(
					make_command("intake_1", "thread_1", {
						engine_id: "engine_1",
						text: "Delete production",
						type: "thread.send_message",
						working_directory: "C:/work",
					}),
					{
						risk: "high",
						resolution: "question",
						assumptions: [],
						question: "Confirm scope",
					},
				),
			);
			const [outbox, intake] = await runtime.runPromise(
				Effect.all([
					Read((database) => database.select().from(OrchestrationOutbox)),
					Read((database) => database.select().from(OrchestrationIntake)),
				]),
			);

			expect(outbox).toHaveLength(0);
			expect(intake).toMatchObject([{ risk: "high", state: "pending" }]);
			expect(accepted.events).toMatchObject([
				{ payload: { type: "intake.assessed", risk: "high", resolution: "question" } },
				{ payload: { type: "interaction.question", state: "requested", source: "intake" } },
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("projects proceeded intake risk and assumptions from durable resolved state", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			migrations_path,
		});

		try {
			await runtime.runPromise(SetupThread("thread_1"));
			await runtime.runPromise(
				Accept(
					make_command("send_1", "thread_1", {
						engine_id: "engine_1",
						text: "Inspect the repository",
						type: "thread.send_message",
						working_directory: "C:/work",
					}),
					{
						assumptions: ["Use the current checked-out branch"],
						risk: "low",
						resolution: "proceed",
					},
				),
			);
			const [session, intake] = await runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* OrchestrationRepository;

					return yield* Effect.all([
						repository.GetSession("thread_1"),
						Read((database) => database.select().from(OrchestrationIntake)),
					]);
				}),
			);

			expect(intake).toMatchObject([
				{
					message_id: "send_1",
					question: null,
					question_id: null,
					risk: "low",
					state: "resolved",
				},
			]);
			expect(session).toMatchObject({
				assumptions: [
					{ assumption: "Use the current checked-out branch", message_id: "send_1" },
				],
				latest_intake: { message_id: "send_1", resolution: "proceed", risk: "low" },
				thread_id: "thread_1",
			});
			expect(session.pending_question).toBeUndefined();
		} finally {
			await runtime.dispose();
		}
	});

	it("reports corrupt persisted intake assumptions instead of treating them as absent", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			migrations_path,
		});

		try {
			await runtime.runPromise(SetupThread("thread_1"));
			await runtime.runPromise(
				Accept(
					make_command("send_corrupt_intake", "thread_1", {
						engine_id: "engine_1",
						text: "Inspect the repository",
						type: "thread.send_message",
						working_directory: "C:/work",
					}),
					{ assumptions: ["Use master"], risk: "low", resolution: "proceed" },
				),
			);
			const outcome = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* OrchestrationRepository;
					yield* database.client.run(
						"UPDATE orchestration_intake SET assumptions_json = '{' WHERE thread_id = 'thread_1'",
					);

					return yield* Effect.exit(repository.GetSession("thread_1"));
				}),
			);

			expect(outcome._tag).toBe("Failure");
			expect(JSON.stringify(outcome)).toContain("OrchestrationFailure");
		} finally {
			await runtime.dispose();
		}
	});

	it("preserves attributed mentions through intake resolution and exact retry", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			migrations_path,
		});
		const origin = { provider: "fixture", reference: "intake_command_1" };

		try {
			await runtime.runPromise(SetupThread("thread_1"));
			const intake = await runtime.runPromise(
				Accept(
					make_command(
						"intake_1",
						"thread_1",
						{
							engine_id: "engine_1",
							mentioned_projects: [
								{
									display_name: "Beta",
									project_id: "project_beta",
									root_path: "C:/work/beta",
								},
							],
							text: "Delete production",
							type: "thread.send_message",
							working_directory: "C:/work",
						},
						{ raw_origin: origin },
					),
					{
						assumptions: [],
						question: "Confirm scope",
						risk: "high",
						resolution: "question",
					},
				),
			);
			const question_event = intake.events[1];
			const question_id =
				question_event?.payload.type === "interaction.question"
					? question_event.payload.question_id
					: "";
			const response = make_command("answer_1", "thread_1", {
				answers: { [question_id]: ["Confirmed"] },
				question_id,
				type: "intake.respond_question",
			});

			const accepted = await runtime.runPromise(Accept(response));
			const duplicate = await runtime.runPromise(Accept(response));
			const [outbox, pending, session, journal] = await runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* OrchestrationRepository;

					return yield* Effect.all([
						Read((database) => database.select().from(OrchestrationOutbox)),
						Read((database) => database.select().from(OrchestrationIntake)),
						repository.GetSession("thread_1"),
						Read((database) => database.select().from(JournalEvents)),
					]);
				}),
			);

			expect(accepted.events).toHaveLength(4);
			expect(accepted.events).toEqual(
				expect.arrayContaining([expect.objectContaining({ raw_origin: origin })]),
			);
			for (const event of accepted.events) {
				expect(event.raw_origin).toEqual(origin);
			}
			expect(duplicate).toMatchObject({
				journal_sequence: accepted.journal_sequence,
				run_id: accepted.run_id,
				status: "duplicate",
			});
			expect(JSON.parse(outbox[0]!.payload_json)).toMatchObject({
				mentioned_projects: [{ project_id: "project_beta" }],
			});
			expect(pending).toMatchObject([{ state: "resolved" }]);
			expect(session.last_routing).toMatchObject({
				message_id: "intake_1",
				outcome: "queued",
				reason: "no_active_run",
				run_id: accepted.run_id,
			});
			expect(
				journal.filter(
					(event) =>
						event.causation_id === response.message_id &&
						event.event_type === "thread.message_routed",
				),
			).toHaveLength(1);
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects extra intake answer keys and resolution while a coordinator run remains active", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			migrations_path,
		});

		try {
			await runtime.runPromise(SetupThread("thread_1"));
			await runtime.runPromise(
				Accept(
					make_command("start_1", "thread_1", {
						engine_id: "engine_1",
						text: "Start",
						type: "thread.send_message",
						working_directory: "C:/work",
					}),
				),
			);
			const intake = await runtime.runPromise(
				Accept(
					make_command("intake_1", "thread_1", {
						engine_id: "engine_1",
						text: "Delete production",
						type: "thread.send_message",
						working_directory: "C:/work",
					}),
					{
						assumptions: [],
						question: "Confirm scope",
						risk: "high",
						resolution: "question",
					},
				),
			);
			const question_event = intake.events[1];
			const question_id =
				question_event?.payload.type === "interaction.question"
					? question_event.payload.question_id
					: "";
			const extra_answers = await runtime.runPromiseExit(
				Accept(
					make_command("answer_extra", "thread_1", {
						answers: { [question_id]: ["Confirmed"], unrelated: ["No"] },
						question_id,
						type: "intake.respond_question",
					}),
				),
			);
			const active_run = await runtime.runPromiseExit(
				Accept(
					make_command("answer_active", "thread_1", {
						answers: { [question_id]: ["Confirmed"] },
						question_id,
						type: "intake.respond_question",
					}),
				),
			);

			expect(extra_answers._tag).toBe("Failure");
			expect(active_run._tag).toBe("Failure");
		} finally {
			await runtime.dispose();
		}
	});

	it("replaces a terminal coordinator run exactly once when intake is resolved", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			migrations_path,
		});

		try {
			await runtime.runPromise(SetupThread("thread_1"));
			const first = await runtime.runPromise(
				Accept(
					make_command("start_1", "thread_1", {
						engine_id: "engine_1",
						text: "Start",
						type: "thread.send_message",
						working_directory: "C:/work",
					}),
				),
			);
			await runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* OrchestrationRepository;

					return yield* repository.RecordObservation({
						_tag: "run_terminal",
						artisan_run_id: first.run_id,
						observation_id: "terminal_1",
						raw: { engine_id: "engine_1", frame: null, transport: "fixture" },
						sequence: 1,
						state: "completed",
					});
				}),
			);
			const intake = await runtime.runPromise(
				Accept(
					make_command("intake_1", "thread_1", {
						engine_id: "engine_1",
						text: "Delete production",
						type: "thread.send_message",
						working_directory: "C:/work",
					}),
					{
						assumptions: [],
						question: "Confirm scope",
						risk: "high",
						resolution: "question",
					},
				),
			);
			const question_event = intake.events[1];
			const question_id =
				question_event?.payload.type === "interaction.question"
					? question_event.payload.question_id
					: "";
			const response = make_command("answer_1", "thread_1", {
				answers: { [question_id]: ["Confirmed"] },
				question_id,
				type: "intake.respond_question",
			});

			const accepted = await runtime.runPromise(Accept(response));
			const duplicate = await runtime.runPromise(Accept(response));
			const runs = await runtime.runPromise(
				Read((database) => database.select().from(OrchestrationRuns)),
			);
			const [thread] = await runtime.runPromise(
				Read((database) => database.select().from(Threads)),
			);

			expect(accepted.run_id).not.toBe(first.run_id);
			expect(duplicate).toMatchObject({ run_id: accepted.run_id, status: "duplicate" });
			expect(runs).toMatchObject([
				{ run_id: first.run_id, status: "completed" },
				{ run_id: accepted.run_id, status: "queued" },
			]);
			expect(thread).toMatchObject({ live_status: "Working" });
		} finally {
			await runtime.dispose();
		}
	});

	it("retries the exact failed root payload once without replaying its user message", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			migrations_path,
		});

		try {
			await runtime.runPromise(SetupThread("thread_1"));
			const failed = await runtime.runPromise(
				Accept(
					make_command("send_1", "thread_1", {
						content: [{ text: "Retry this exact request", type: "text" }],
						engine_id: "engine_1",
						mentioned_projects: [
							{
								display_name: "Workspace",
								project_id: "project_1",
								root_path: "C:/work",
							},
						],
						text: "Retry this exact request",
						type: "thread.send_message",
						working_directory: "C:/work",
					}),
				),
			);
			await runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* OrchestrationRepository;

					yield* repository.RecordObservation({
						_tag: "run_terminal",
						artisan_run_id: failed.run_id,
						observation_id: "failed_1",
						raw: { engine_id: "engine_1", frame: null, transport: "fixture" },
						sequence: 1,
						state: "failed",
					});
				}),
			);
			const retry = make_command("retry_1", "thread_1", {
				run_id: failed.run_id,
				type: "run.retry",
			});
			const accepted = await runtime.runPromise(Accept(retry));
			const duplicate = await runtime.runPromise(Accept(retry));
			const persisted = await runtime.runPromise(
				Read((database) =>
					Effect.all({
						events: database.select().from(JournalEvents),
						messages: database.select().from(OrchestrationMessages),
						outbox: database.select().from(OrchestrationOutbox),
						runs: database.select().from(OrchestrationRuns),
						threads: database.select().from(Threads),
					}),
				),
			);
			const source_outbox = persisted.outbox.find((entry) => entry.command_id === "send_1");
			const retry_outbox = persisted.outbox.find((entry) => entry.command_id === "retry_1");

			expect(accepted).toMatchObject({ status: "accepted" });
			expect(duplicate).toMatchObject({ run_id: accepted.run_id, status: "duplicate" });
			expect(persisted.runs).toMatchObject([
				{ run_id: failed.run_id, status: "failed" },
				{
					engine_id: "engine_1",
					model_id: null,
					run_id: accepted.run_id,
					status: "queued",
					working_directory: "C:/work",
				},
			]);
			expect(retry_outbox).toMatchObject({
				kind: "start",
				run_id: accepted.run_id,
				status: "pending",
			});
			expect(retry_outbox?.payload_json).toBe(source_outbox?.payload_json);
			expect(persisted.messages).toMatchObject([
				{ command_id: "send_1", delivery: "queued", message_id: "send_1" },
				{
					command_id: "retry_1",
					delivery: "queued",
					message_id: "retry_1",
					run_id: accepted.run_id,
					text: "Retry this exact request",
				},
			]);
			expect(
				persisted.events.filter(
					(event) => JSON.parse(event.payload_json).type === "thread.message_queued",
				),
			).toHaveLength(1);
			expect(
				persisted.events.filter((event) => event.correlation_id === "retry_1"),
			).toMatchObject([{ payload_json: expect.stringContaining('"type":"run.lifecycle"') }]);
			expect(
				persisted.threads.find((thread) => thread.thread_id === "thread_1"),
			).toMatchObject({
				live_status: "Working",
			});

			const stale = await runtime.runPromiseExit(
				Accept(
					make_command("retry_stale", "thread_1", {
						run_id: failed.run_id,
						type: "run.retry",
					}),
				),
			);
			const nonfailed = await runtime.runPromiseExit(
				Accept(
					make_command("retry_queued", "thread_1", {
						run_id: accepted.run_id,
						type: "run.retry",
					}),
				),
			);
			const unknown = await runtime.runPromiseExit(
				Accept(
					make_command("retry_missing", "thread_1", {
						run_id: "run_missing",
						type: "run.retry",
					}),
				),
			);

			expect(stale._tag).toBe("Failure");
			expect(nonfailed._tag).toBe("Failure");
			expect(unknown._tag).toBe("Failure");
		} finally {
			await runtime.dispose();
		}
	});

	it("releases requested interactions when the run terminates and rejects late responses", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			migrations_path,
		});

		try {
			await runtime.runPromise(SetupThread("thread_1"));
			const started = await runtime.runPromise(
				Accept(
					make_command("start_1", "thread_1", {
						engine_id: "engine_1",
						text: "Start",
						type: "thread.send_message",
						working_directory: "C:/work",
					}),
				),
			);
			await runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* OrchestrationRepository;

					yield* repository.RecordObservation({
						_tag: "approval",
						approval_id: "approval_1",
						artisan_run_id: started.run_id,
						description: "Run the build",
						observation_id: "approval_requested",
						raw: { engine_id: "engine_1", frame: null, transport: "fixture" },
						request: { command: "pnpm build", kind: "command" },
						sequence: 1,
						state: "requested",
					});
					yield* repository.RecordObservation({
						_tag: "run_terminal",
						artisan_run_id: started.run_id,
						observation_id: "run_cancelled",
						raw: { engine_id: "engine_1", frame: null, transport: "fixture" },
						sequence: 2,
						state: "cancelled",
					});
				}),
			);
			const interactions = await runtime.runPromise(
				Read((database) => database.select().from(OrchestrationInteractions)),
			);
			const late_response = await runtime.runPromiseExit(
				Accept(
					make_command("respond_1", "thread_1", {
						approval_id: "approval_1",
						approved: true,
						type: "run.respond_approval",
					}),
				),
			);

			expect(interactions).toMatchObject([
				{ interaction_id: "approval_1", state: "cancelled" },
			]);
			expect(late_response._tag).toBe("Failure");
		} finally {
			await runtime.dispose();
		}
	});

	it("heals interactions left requested by a run that terminated before release existed", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			migrations_path,
		});

		try {
			await runtime.runPromise(SetupThread("thread_1"));
			const started = await runtime.runPromise(
				Accept(
					make_command("start_1", "thread_1", {
						engine_id: "engine_1",
						text: "Start",
						type: "thread.send_message",
						working_directory: "C:/work",
					}),
				),
			);
			await runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* OrchestrationRepository;

					yield* repository.RecordObservation({
						_tag: "approval",
						approval_id: "approval_1",
						artisan_run_id: started.run_id,
						description: "Run the build",
						observation_id: "approval_requested",
						raw: { engine_id: "engine_1", frame: null, transport: "fixture" },
						request: { command: "pnpm build", kind: "command" },
						sequence: 1,
						state: "requested",
					});
				}),
			);
			/** A pre-release database: the only run ended without touching its interactions. */
			await runtime.runPromise(
				Read((database) => database.update(OrchestrationRuns).set({ status: "cancelled" })),
			);
			await runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* OrchestrationRepository;

					yield* repository.ClaimNativeRecoveries();
				}),
			);
			const [interactions, availability] = await runtime.runPromise(
				Effect.all([
					Read((database) => database.select().from(OrchestrationInteractions)),
					Effect.gen(function* () {
						const read_model = yield* ConversationReadModel;

						return yield* read_model.ReadSnapshot("thread_1");
					}),
				]),
			);

			expect(interactions).toMatchObject([
				{ interaction_id: "approval_1", state: "cancelled" },
			]);
			expect(availability.status).toBe("available");
			if (availability.status !== "available") return;
			expect(
				availability.snapshot.items.find((item) => item.type === "approval"),
			).toMatchObject({
				interaction_id: "approval_1",
				resolution: "Cancelled",
				state: "cancelled",
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("replays an exact intake response after repository restart without a second run", async () => {
		const database_path = await make_database_path();
		const first_runtime = make_backend_runtime({ database_path, migrations_path });
		let response: AuthoritativeCommandEnvelope | undefined;
		let accepted_run_id: string | undefined;

		try {
			await first_runtime.runPromise(SetupThread("thread_1"));
			const intake = await first_runtime.runPromise(
				Accept(
					make_command("intake_1", "thread_1", {
						engine_id: "engine_1",
						text: "Delete production",
						type: "thread.send_message",
						working_directory: "C:/work",
					}),
					{
						assumptions: [],
						question: "Confirm scope",
						risk: "high",
						resolution: "question",
					},
				),
			);
			const question_event = intake.events[1];
			const question_id =
				question_event?.payload.type === "interaction.question"
					? question_event.payload.question_id
					: "";
			response = make_command("answer_1", "thread_1", {
				answers: { [question_id]: ["Confirmed"] },
				question_id,
				type: "intake.respond_question",
			});
			accepted_run_id = (await first_runtime.runPromise(Accept(response))).run_id;
		} finally {
			await first_runtime.dispose();
		}

		const restarted_runtime = make_backend_runtime({ database_path, migrations_path });
		try {
			const duplicate = await restarted_runtime.runPromise(Accept(response!));
			const runs = await restarted_runtime.runPromise(
				Read((database) => database.select().from(OrchestrationRuns)),
			);

			expect(duplicate).toMatchObject({ run_id: accepted_run_id, status: "duplicate" });
			expect(runs).toHaveLength(1);
			expect(runs[0]?.run_id).toBe(accepted_run_id);
		} finally {
			await restarted_runtime.dispose();
		}
	});

	it("erases pending intake rows with their thread", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			migrations_path,
		});

		try {
			await runtime.runPromise(SetupThread("thread_1"));
			await runtime.runPromise(
				Accept(
					make_command("intake_1", "thread_1", {
						engine_id: "engine_1",
						text: "Delete production",
						type: "thread.send_message",
						working_directory: "C:/work",
					}),
					{
						assumptions: [],
						question: "Confirm scope",
						risk: "high",
						resolution: "question",
					},
				),
			);
			await runtime.runPromise(
				Effect.gen(function* () {
					return yield* (yield* ThreadErasure).CleanupExpired(
						"9999-01-01T00:00:00.000Z",
						"2026-07-18T12:00:00.000Z",
					);
				}),
			);
			const intake = await runtime.runPromise(
				Read((database) => database.select().from(OrchestrationIntake)),
			);

			expect(intake).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});

	it("persists an exact Codex-only session policy while ignoring the legacy workflow column", async () => {
		const database_path = await make_database_path();
		const policy = {
			engine_id: "codex" as const,
			model: "gpt-5.3-codex",
			permission: "supervised",
			permission_mode: "on_request" as const,
			reasoning_effort: "high" as const,
			sandbox_mode: "workspace_write" as const,
			service_tier: "standard" as const,
			strict_clarification: true,
			web_search_enabled: true,
		};
		const command = make_command("policy_1", "thread_1", {
			type: "thread.session_policy.update",
			policy,
		});
		const first_runtime = make_backend_runtime({ database_path, migrations_path });

		try {
			await first_runtime.runPromise(SetupFreshThread("thread_1"));
			const accepted = await first_runtime.runPromise(Accept(command));
			const duplicate = await first_runtime.runPromise(Accept(command));
			await first_runtime.runPromise(
				Read((database) =>
					database.run(
						"UPDATE orchestration_coordinators SET policy_workflow_mode = 'plan' WHERE thread_id = 'thread_1'",
					),
				),
			);
			const persisted = await first_runtime.runPromise(
				Effect.gen(function* () {
					return yield* (yield* OrchestrationRepository).GetSession("thread_1");
				}),
			);

			expect(accepted.events).toMatchObject([
				{
					payload: {
						type: "thread.session_policy.updated",
						policy,
					},
				},
			]);
			expect(duplicate).toMatchObject({
				journal_sequence: accepted.journal_sequence,
				status: "duplicate",
			});
			expect(persisted.policy).toEqual(policy);
		} finally {
			await first_runtime.dispose();
		}

		const restarted_runtime = make_backend_runtime({ database_path, migrations_path });
		try {
			const session = await restarted_runtime.runPromise(
				Effect.gen(function* () {
					return yield* (yield* OrchestrationRepository).GetSession("thread_1");
				}),
			);
			const journal = await restarted_runtime.runPromise(
				Read((database) => database.select().from(JournalEvents)),
			);
			expect(
				journal.some((event) => event.event_type === "thread.session_policy.updated"),
			).toBe(true);
			expect(session.policy).toEqual(policy);
			await restarted_runtime.runPromise(
				Effect.gen(function* () {
					return yield* (yield* ThreadErasure).CleanupExpired(
						"9999-01-01T00:00:00.000Z",
						"2026-07-18T12:00:00.000Z",
					);
				}),
			);
			const erased = await restarted_runtime.runPromise(
				Effect.gen(function* () {
					return yield* (yield* OrchestrationRepository).GetSession("thread_1");
				}),
			);
			expect(erased.policy).toMatchObject({ engine_id: "codex", reasoning_effort: "medium" });
		} finally {
			await restarted_runtime.dispose();
		}
	});

	it("rejects corrupt persisted session policy rows instead of casting them into launches", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			migrations_path,
		});

		try {
			await runtime.runPromise(SetupThread("thread_1"));
			await runtime.runPromise(
				Accept(
					make_command("policy_corrupt", "thread_1", {
						policy: {
							engine_id: "codex",
							permission_mode: "on_request",
							reasoning_effort: "medium",
							sandbox_mode: "workspace_write",
							service_tier: "standard",
							strict_clarification: false,
							web_search_enabled: false,
						},
						type: "thread.session_policy.update",
					}),
				),
			);
			await runtime.runPromise(
				Read((database) =>
					database.run(
						"UPDATE orchestration_coordinators SET policy_reasoning_effort = 'invalid' WHERE thread_id = 'thread_1'",
					),
				),
			);

			await expect(
				runtime.runPromise(
					Effect.gen(function* () {
						return yield* (yield* OrchestrationRepository).GetSessionPolicy("thread_1");
					}),
				),
			).rejects.toMatchObject({ _tag: "OrchestrationFailure" });
		} finally {
			await runtime.dispose();
		}
	});
	it("accepts an exact retry without a run id and rejects changed envelopes", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			migrations_path,
		});
		const command = make_command(
			"send_1",
			"thread_1",
			{
				engine_id: "engine_1",
				mentioned_projects: [
					{
						display_name: "Beta",
						project_id: "project_beta",
						root_path: "C:/work/beta",
					},
				],
				text: "Start",
				type: "thread.send_message",
				working_directory: "C:/work",
			},
			{ raw_origin: { provider: "fixture", reference: "command_1" } },
		);

		try {
			await runtime.runPromise(SetupThread("thread_1"));

			const accepted = await runtime.runPromise(Accept(command));
			const duplicate = await runtime.runPromise(Accept(command));
			const changed = await runtime.runPromiseExit(
				Accept({ ...command, run_id: "run_changed" }),
			);
			const persisted = await runtime.runPromise(
				Read((database) => database.select().from(JournalCommands)),
			);

			expect(duplicate).toMatchObject({
				journal_sequence: accepted.journal_sequence,
				run_id: accepted.run_id,
				status: "duplicate",
			});
			expect(duplicate.events).toEqual(
				expect.arrayContaining([
					expect.objectContaining({
						payload: expect.objectContaining({
							mentioned_projects: [
								{
									display_name: "Beta",
									project_id: "project_beta",
									root_path: "C:/work/beta",
								},
							],
							type: "thread.message_queued",
						}),
						raw_origin: { provider: "fixture", reference: "command_1" },
					}),
				]),
			);
			expect(changed._tag).toBe("Failure");
			expect(
				persisted.find((entry) => entry.message_id === command.message_id),
			).toMatchObject({
				assigned_run_id: accepted.run_id,
				run_id: null,
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects a run id owned by another coordinator thread", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			migrations_path,
		});

		try {
			await runtime.runPromise(SetupThread("thread_1"));
			await runtime.runPromise(SetupThread("thread_2"));

			const first = await runtime.runPromise(
				Accept(
					make_command("send_1", "thread_1", {
						engine_id: "engine_1",
						text: "Start one",
						type: "thread.send_message",
						working_directory: "C:/one",
					}),
				),
			);
			await runtime.runPromise(
				Accept(
					make_command("send_2", "thread_2", {
						engine_id: "engine_1",
						text: "Start two",
						type: "thread.send_message",
						working_directory: "C:/two",
					}),
				),
			);
			const rejected = await runtime.runPromiseExit(
				Accept(
					make_command(
						"cancel_1",
						"thread_2",
						{ type: "run.cancel" },
						{ run_id: first.run_id },
					),
				),
			);

			expect(rejected._tag).toBe("Failure");
		} finally {
			await runtime.dispose();
		}
	});

	it("uses the run watermark for duplicate filtering and keeps terminal runs closed", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			migrations_path,
		});

		try {
			await runtime.runPromise(SetupThread("thread_1"));
			const accepted = await runtime.runPromise(
				Accept(
					make_command("send_1", "thread_1", {
						engine_id: "engine_1",
						text: "Start",
						type: "thread.send_message",
						working_directory: "C:/work",
					}),
				),
			);
			const terminal: EngineObservation = {
				_tag: "run_terminal",
				artisan_run_id: accepted.run_id,
				observation_id: "terminal_1",
				raw: {
					engine_id: "engine_1",
					frame: { type: "terminal", value: "complete" },
					native_id: "native-terminal",
					native_method: "thread.completed",
					protocol_version: "1.0",
					raw_frame_base64: "AAEC/w==",
					transport: "stdio-jsonl",
				},
				sequence: 2,
				state: "completed",
			};
			const delayed: EngineObservation = {
				_tag: "run_state",
				artisan_run_id: accepted.run_id,
				observation_id: "running_late",
				raw: {
					engine_id: "engine_1",
					frame: { state: "running" },
					transport: "stdio-jsonl",
				},
				sequence: 1,
				state: "running",
			};

			const observations = await runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* OrchestrationRepository;

					const completed = yield* repository.RecordObservation(terminal);
					const duplicate = yield* repository.RecordObservation(terminal);
					const late = yield* repository.RecordObservation(delayed);

					return { completed, duplicate, late };
				}),
			);
			const persisted = await runtime.runPromise(
				Read((database) => database.select().from(OrchestrationRawObservations)),
			);
			const runs = await runtime.runPromise(
				Read((database) => database.select().from(OrchestrationRuns)),
			);

			expect(observations).toMatchObject({ duplicate: [], late: [] });
			expect(observations.completed).toHaveLength(1);
			expect(persisted).toEqual([]);
			expect(runs.find((entry) => entry.run_id === accepted.run_id)).toMatchObject({
				last_observation_sequence: 2,
				status: "completed",
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("settles a resumed run with no fresh provider observation without touching progressed work", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			migrations_path,
		});
		try {
			await runtime.runPromise(SetupThread("thread_1"));
			const accepted = await runtime.runPromise(
				Accept(
					make_command("resume_stalled", "thread_1", {
						engine_id: "engine_1",
						text: "Resume without an event stream",
						type: "thread.send_message",
						working_directory: "C:/work",
					}),
				),
			);
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* OrchestrationRepository;
					yield* repository.MarkRunStarted(accepted.run_id);

					return yield* repository.FailRecoveredRun(accepted.run_id, -1);
				}),
			);
			const [runs, threads] = await runtime.runPromise(
				Read((database) =>
					Effect.all([
						database.select().from(OrchestrationRuns),
						database.select().from(Threads),
					]),
				),
			);

			expect(result).toBe(true);
			expect(runs.find((run) => run.run_id === accepted.run_id)).toMatchObject({
				status: "failed",
			});
			expect(threads.find((thread) => thread.thread_id === "thread_1")).toMatchObject({
				live_status: "Failed to complete",
			});

			const progressed = await runtime.runPromise(
				Accept(
					make_command("resume_progressed", "thread_1", {
						engine_id: "engine_1",
						text: "Resume with a real provider observation",
						type: "thread.send_message",
						working_directory: "C:/work",
					}),
				),
			);
			const protected_result = await runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* OrchestrationRepository;
					yield* repository.MarkRunStarted(progressed.run_id);
					yield* repository.RecordObservation({
						_tag: "agent_message_delta",
						artisan_run_id: progressed.run_id,
						delta: "Still alive",
						item_id: "assistant_progress",
						observation_id: "resume_progress",
						phase: "unspecified",
						raw: { engine_id: "engine_1", frame: null, transport: "fixture" },
						sequence: 0,
						turn_id: "turn_progress",
					});

					return yield* repository.FailRecoveredRun(progressed.run_id, -1);
				}),
			);
			const protected_runs = await runtime.runPromise(
				Read((database) => database.select().from(OrchestrationRuns)),
			);

			expect(protected_result).toBe(false);
			expect(protected_runs.find((run) => run.run_id === progressed.run_id)).toMatchObject({
				status: "running",
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("repairs a stale projection repeatedly from the exact active root run", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			migrations_path,
		});

		try {
			await runtime.runPromise(SetupThread("thread_1"));
			const accepted = await runtime.runPromise(
				Accept(
					make_command("repair_status", "thread_1", {
						engine_id: "engine_1",
						text: "Finish before repair",
						type: "thread.send_message",
						working_directory: "C:/work",
					}),
				),
			);
			await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* OrchestrationRepository;

					yield* repository.RecordObservation({
						_tag: "run_terminal",
						artisan_run_id: accepted.run_id,
						observation_id: "repair_terminal",
						raw: { engine_id: "engine_1", frame: null, transport: "fixture" },
						sequence: 0,
						state: "completed",
					});
					yield* database.client.insert(OrchestrationRuns).values({
						agent_id: "agent_historical",
						created_at: "2026-07-10T09:00:00.000Z",
						engine_id: "engine_1",
						last_observation_sequence: 0,
						run_id: "run_historical_failed",
						status: "failed",
						thread_id: "thread_1",
						updated_at: "2099-01-01T00:00:00.000Z",
						working_directory: "C:/work",
					});

					for (const attempt of [1, 2]) {
						yield* database.client.update(Threads).set({ live_status: "Working" });
						yield* repository.MarkInterrupted();
						const [thread] = yield* database.client
							.select({ live_status: Threads.live_status })
							.from(Threads);

						expect(thread?.live_status, `repair attempt ${attempt}`).toBe("Complete");
					}
				}),
			);
		} finally {
			await runtime.dispose();
		}
	});

	it("marks dispatching work undeliverable and interrupts its queued run on recovery", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			migrations_path,
		});

		try {
			await runtime.runPromise(SetupThread("thread_1"));
			const accepted = await runtime.runPromise(
				Accept(
					make_command("send_1", "thread_1", {
						engine_id: "engine_1",
						text: "Start",
						type: "thread.send_message",
						working_directory: "C:/work",
					}),
				),
			);
			await runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* OrchestrationRepository;

					expect(yield* repository.ClaimOutbox("send_1")).toBe(true);
					yield* repository.MarkInterrupted();
				}),
			);
			const outboxes = await runtime.runPromise(
				Read((database) => database.select().from(OrchestrationOutbox)),
			);
			const runs = await runtime.runPromise(
				Read((database) => database.select().from(OrchestrationRuns)),
			);

			expect(outboxes.find((entry) => entry.command_id === "send_1")).toMatchObject({
				status: "undeliverable",
			});
			expect(runs.find((entry) => entry.run_id === accepted.run_id)).toMatchObject({
				status: "interrupted",
			});
		} finally {
			await runtime.dispose();
		}
	});
});
