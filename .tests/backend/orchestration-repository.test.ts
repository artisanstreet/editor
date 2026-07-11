import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { EngineObservation } from "@artisan/engines";
import type { CommandEnvelope } from "@artisan/protocol";
import { make_backend_runtime } from "@artisan/backend";

import { OrchestrationRepository } from "../../modules/backend/src/persistence/orchestration-repository";
import {
	JournalCommands,
	OrchestrationOutbox,
	OrchestrationRawObservations,
	OrchestrationRuns,
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

const Accept = (command: CommandEnvelope) =>
	Effect.gen(function* () {
		const repository = yield* OrchestrationRepository;

		return yield* repository.Accept(command, false);
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
