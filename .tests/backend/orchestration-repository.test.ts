import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { EngineObservation } from "@artisan/engines";
import type { CommandEnvelope } from "@artisan/protocol";
import { make_backend_runtime, ThreadErasure } from "@artisan/backend";

import { OrchestrationRepository } from "../../modules/backend/src/persistence/orchestration-repository";
import type { IntakeAssessment } from "../../modules/backend/src/orchestration/intake-policy";
import {
	JournalCommands,
	JournalEvents,
	OrchestrationOutbox,
	OrchestrationIntake,
	OrchestrationRawObservations,
	OrchestrationRuns,
	SurfaceItems,
	Threads,
} from "../../modules/backend/src/persistence/schema";
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
	payload: CommandEnvelope["payload"],
	options: Partial<Pick<CommandEnvelope, "raw_origin" | "run_id">> = {},
): CommandEnvelope {
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

const Accept = (command: CommandEnvelope, intake?: IntakeAssessment) =>
	Effect.gen(function* () {
		const repository = yield* OrchestrationRepository;

		return yield* repository.Accept(command, false, intake);
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
	it("projects an agent message delta while retaining its raw observation", async () => {
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
						observation_id: "delta_1",
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
			expect(raw).toEqual([expect.objectContaining({ observation_id: "delta_1" })]);
			expect(surfaces).toEqual([
				expect.objectContaining({
					category: "work",
					kind: "message",
					observation_id: "delta_1",
					run_id: accepted.run_id,
					thread_id: "thread_1",
				}),
			]);
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

			expect(accepted.run_id).not.toBe(first.run_id);
			expect(duplicate).toMatchObject({ run_id: accepted.run_id, status: "duplicate" });
			expect(runs).toMatchObject([
				{ run_id: first.run_id, status: "completed" },
				{ run_id: accepted.run_id, status: "queued" },
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("replays an exact intake response after repository restart without a second run", async () => {
		const database_path = await make_database_path();
		const first_runtime = make_backend_runtime({ database_path, migrations_path });
		let response: CommandEnvelope | undefined;
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

	it("persists an exact Codex-only session policy across restart and erasure", async () => {
		const database_path = await make_database_path();
		const policy = {
			engine_id: "codex" as const,
			model: "gpt-5.3-codex",
			permission_mode: "on_request" as const,
			reasoning_effort: "high" as const,
			sandbox_mode: "workspace_write" as const,
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

	it("durably records raw observations before filtering and keeps terminal runs closed", async () => {
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
			expect(persisted).toEqual(
				expect.arrayContaining([
					expect.objectContaining({
						frame_json: '{"type":"terminal","value":"complete"}',
						native_id: "native-terminal",
						raw_frame_base64: "AAEC/w==",
						sequence: 2,
					}),
				]),
			);
			expect(runs.find((entry) => entry.run_id === accepted.run_id)).toMatchObject({
				status: "completed",
			});
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
