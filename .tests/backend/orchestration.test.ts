import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Latch, Layer, PubSub, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { CommandEnvelope, HelloEnvelope } from "@artisan/protocol";
import type {
	Engine,
	EngineCommand,
	EngineObservation,
	EngineOpenInput,
	EngineRun,
} from "@artisan/engines";
import {
	AgentOrchestrator,
	ExternalWaitDispatchScheduler,
	make_backend_runtime,
	ProtocolServer,
	type ProtocolConnection,
} from "@artisan/backend";

import { Database } from "../../modules/backend/src/persistence/database";
import { JournalNotifier } from "../../modules/backend/src/persistence/journal-notifier";
import {
	OrchestrationCoordinators,
	OrchestrationOutbox,
	OrchestrationRuns,
	Threads,
} from "../../modules/backend/src/persistence/schema";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const ExternalWaitDispatchSchedulerDisabled = Layer.succeed(ExternalWaitDispatchScheduler, {
	Schedule: () => Effect.never,
});

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-orchestration-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

function make_engine() {
	const commands: EngineCommand[] = [];
	const inputs: EngineOpenInput[] = [];
	const commands_latch = Latch.makeUnsafe();
	const opened_latch = Latch.makeUnsafe();
	let events_consumed = 0;
	let opened = 0;
	let resume_state: "supported" | "unsupported" = "supported";
	const capabilities = {
		...Object.fromEntries(
			[
				"approval",
				"auth",
				"cancel",
				"close",
				"events",
				"global_guidance",
				"harness_context",
				"model_selection",
				"native_tools",
				"probe",
				"question",
				"raw_frames",
				"start",
				"steer",
				"subagents",
			].map((name) => [name, { state: "supported" as const }]),
		),
		get resume() {
			return { state: resume_state };
		},
	} as Engine["Descriptor"]["capabilities"];

	const Open = (input: EngineOpenInput) =>
		Effect.gen(function* () {
			opened += 1;
			inputs.push(input);
			yield* opened_latch.open;

			const observations: ReadonlyArray<EngineObservation> = [
				{
					_tag: "approval",
					approval_id: "approval_1",
					artisan_run_id: input.artisan_run_id,
					description: "Approve deterministic action",
					observation_id: "approval_observation",
					raw: { engine_id: "deterministic", frame: "approval", transport: "test" },
					sequence: 1,
					state: "requested",
				},
				{
					_tag: "question",
					artisan_run_id: input.artisan_run_id,
					observation_id: "question_observation",
					question_id: "question_1",
					raw: { engine_id: "deterministic", frame: "question", transport: "test" },
					sequence: 2,
					state: "requested",
					text: "Continue?",
				},
			];
			const run: EngineRun = {
				artisan_run_id: input.artisan_run_id,
				Closed: Effect.never,
				Events: Stream.unwrap(
					Effect.sync(() => {
						events_consumed += 1;

						return Stream.concat(Stream.fromIterable(observations), Stream.never);
					}),
				),
				native_thread_id: `native:${input.artisan_run_id}`,
				resume_token: { native_thread_id: `native:${input.artisan_run_id}` },
				Send: (command) =>
					Effect.gen(function* () {
						commands.push(command);

						if (commands.length === 2) {
							yield* commands_latch.open;
						}
					}),
			};

			return run;
		});

	return {
		commands,
		commands_latch,
		engine: {
			Descriptor: {
				capabilities,
				display_name: "Deterministic backend engine",
				id: "deterministic",
				transport: "test",
			},
			Open,
			Probe: () => Effect.die("Probe is not used by orchestration tests"),
		} satisfies Engine,
		events_consumed: () => events_consumed,
		inputs,
		opened: () => opened,
		opened_latch,
		set_resume_state: (state: "supported" | "unsupported") => {
			resume_state = state;
		},
	};
}

const SeedResumeWork = Effect.gen(function* () {
	const database = yield* Database;
	const created_at = "2026-07-10T08:00:00.000Z";
	const payload = {
		engine_id: "deterministic",
		text: "Continue after the external review changed.",
		type: "thread.send_message" as const,
		working_directory: "C:/work",
	};

	yield* database.client.insert(Threads).values({
		created_at,
		thread_id: "thread_1",
		title: "Resume orchestration",
		updated_at: created_at,
	});
	yield* database.client.insert(OrchestrationCoordinators).values({
		active_run_id: "resume_run",
		agent_id: "agent_1",
		created_at,
		display_name: "Primary coordinator",
		engine_id: "deterministic",
		native_resume_json: '{"native_thread_id":"native:resume_run"}',
		native_thread_id: "native:resume_run",
		role: "primary",
		thread_id: "thread_1",
		updated_at: created_at,
	});
	yield* database.client.insert(OrchestrationRuns).values({
		agent_id: "agent_1",
		created_at,
		engine_id: "deterministic",
		native_resume_json: '{"native_thread_id":"native:resume_run"}',
		native_thread_id: "native:resume_run",
		open_mode: "resume",
		run_id: "resume_run",
		status: "queued",
		thread_id: "thread_1",
		updated_at: created_at,
		working_directory: "C:/work",
	});
	yield* database.client.insert(OrchestrationOutbox).values({
		agent_id: "agent_1",
		command_id: "resume_command",
		created_at,
		kind: "resume",
		payload_json: JSON.stringify(payload),
		run_id: "resume_run",
		status: "pending",
		thread_id: "thread_1",
		updated_at: created_at,
	});
});

function make_hello(): HelloEnvelope {
	return {
		kind: "hello",
		message_id: "hello_1",
		origin: "frontend",
		payload: { event_cursors: [], last_journal_sequence: 0, supported_protocol_versions: [1] },
		schema_version: 1,
		sent_at: "2026-07-10T08:00:00.000Z",
	};
}

function make_command(message_id: string, payload: CommandEnvelope["payload"]): CommandEnvelope {
	return {
		kind: "command",
		message_id,
		origin: "frontend",
		payload,
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-10T08:00:00.000Z",
		thread_id: "thread_1",
	};
}

function take_outbound(connection: ProtocolConnection, count: number) {
	return connection.Outbound.pipe(Stream.take(count), Stream.runCollect);
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("single coordinator orchestration", () => {
	it("accepts durable work before one engine consumer and routes durable interaction responses", async () => {
		const database_path = await make_database_path();
		const deterministic = make_engine();
		const runtime = make_backend_runtime({
			database_path,
			engines: [deterministic.engine],
			external_wait_dispatch_scheduler: ExternalWaitDispatchSchedulerDisabled,
			migrations_path,
		});

		try {
			const result = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const server = yield* ProtocolServer;
						const connection = yield* server.Open;

						yield* connection.Receive(make_hello());
						yield* take_outbound(connection, 2);
						yield* connection.Receive(
							make_command("create_1", {
								title: "Orchestration",
								type: "thread.create",
							}),
						);
						yield* take_outbound(connection, 2);
						yield* connection.Receive(
							make_command("send_1", {
								engine_id: "deterministic",
								text: "Start work",
								type: "thread.send_message",
								working_directory: "C:/work",
							}),
						);

						const accepted = yield* take_outbound(connection, 3);
						const interactions = yield* connection.Outbound.pipe(
							Stream.filter(
								(envelope) =>
									envelope.kind === "event" &&
									(envelope.payload.type === "interaction.approval" ||
										envelope.payload.type === "interaction.question") &&
									envelope.payload.state === "requested",
							),
							Stream.take(2),
							Stream.runCollect,
						);

						yield* connection.Receive(
							make_command("approval_response_1", {
								approval_id: "approval_1",
								approved: true,
								type: "run.respond_approval",
							}),
						);
						yield* connection.Receive(
							make_command("question_response_1", {
								answers: { question_1: ["yes"] },
								type: "run.respond_question",
							}),
						);
						const responses = yield* connection.Outbound.pipe(
							Stream.filter(
								(envelope) =>
									envelope.kind === "command.receipt" &&
									(envelope.correlation_id === "approval_response_1" ||
										envelope.correlation_id === "question_response_1"),
							),
							Stream.take(2),
							Stream.runCollect,
						);

						return { accepted, interactions, responses };
					}),
				),
			);

			expect(result.accepted).toMatchObject([
				{ kind: "command.receipt", payload: { status: "accepted" } },
				{ kind: "event", payload: { type: "thread.message_queued" } },
				{ kind: "event", payload: { state: "queued", type: "run.lifecycle" } },
			]);

			await runtime.runPromise(deterministic.opened_latch.await);
			await runtime.runPromise(deterministic.commands_latch.await);

			expect(deterministic.opened()).toBe(1);
			expect(deterministic.events_consumed()).toBe(1);
			expect(result.interactions).toHaveLength(2);
			expect(result.responses).toHaveLength(2);
			expect(deterministic.commands.map((command) => command._tag)).toEqual([
				"respond_approval",
				"respond_question",
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("opens durable resume work with its exact stored provider token", async () => {
		const deterministic = make_engine();
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			engines: [deterministic.engine],
			external_wait_dispatch_scheduler: ExternalWaitDispatchSchedulerDisabled,
			migrations_path,
		});

		try {
			await runtime.runPromise(
				Effect.gen(function* () {
					const orchestrator = yield* AgentOrchestrator;
					yield* SeedResumeWork;
					yield* orchestrator.NotifyWorkAvailable;
					yield* deterministic.opened_latch.await;
				}),
			);

			expect(deterministic.inputs).toEqual([
				expect.objectContaining({
					_tag: "resume",
					artisan_run_id: "resume_run",
					next_text: "Continue after the external review changed.",
					resume_token: { native_thread_id: "native:resume_run" },
					working_directory: "C:/work",
				}),
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("fails a durable native resume when engine capability changes before open", async () => {
		const deterministic = make_engine();
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			engines: [deterministic.engine],
			external_wait_dispatch_scheduler: ExternalWaitDispatchSchedulerDisabled,
			migrations_path,
		});

		try {
			await runtime.runPromise(SeedResumeWork);
			deterministic.set_resume_state("unsupported");

			const failed_run = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const database = yield* Database;
						const notifier = yield* JournalNotifier;
						const orchestrator = yield* AgentOrchestrator;
						const subscription = yield* notifier.Subscribe;

						yield* orchestrator.NotifyWorkAvailable;
						yield* PubSub.take(subscription);
						yield* PubSub.take(subscription);

						const [run] = yield* database.client
							.select()
							.from(OrchestrationRuns)
							.limit(1);

						return run;
					}),
				),
			);

			expect(deterministic.opened()).toBe(0);
			expect(deterministic.inputs).toEqual([]);
			expect(failed_run?.status).toBe("failed");
		} finally {
			await runtime.dispose();
		}
	});
});
