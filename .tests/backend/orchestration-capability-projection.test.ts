import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { make_backend_runtime } from "@artisan/backend";
import type {
	CapabilityInvocationUpdatedEvent,
	CommandEnvelope,
	EngineNativeActionObservedEvent,
	EventEnvelope,
} from "@artisan/protocol";
import type { EngineObservation } from "@artisan/engines";

import { Database } from "../../modules/backend/src/persistence/database";
import { OrchestrationRepository } from "../../modules/backend/src/persistence/orchestration-repository";
import {
	JournalEvents,
	OrchestrationRawObservations,
	Threads,
} from "../../modules/backend/src/persistence/schema";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];

function is_capability_event(
	event: EventEnvelope,
): event is EventEnvelope & { readonly payload: CapabilityInvocationUpdatedEvent } {
	return event.payload.type === "capability.invocation.updated";
}

function is_native_action_event(
	event: EventEnvelope,
): event is EventEnvelope & { readonly payload: EngineNativeActionObservedEvent } {
	return event.payload.type === "engine.native_action.observed";
}

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-capability-projection-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

const SetupThread = (thread_id: string) =>
	Effect.gen(function* () {
		const database = yield* Database;

		yield* database.client.insert(Threads).values({
			created_at: "2026-07-16T10:00:00.000Z",
			thread_id,
			title: thread_id,
			updated_at: "2026-07-16T10:00:00.000Z",
		});
	});

const OpenRun = (thread_id: string, engine_id = "engine_1") =>
	Effect.gen(function* () {
		const repository = yield* OrchestrationRepository;
		const command: CommandEnvelope = {
			kind: "command",
			message_id: `send:${thread_id}`,
			origin: "frontend",
			payload: {
				engine_id,
				text: "Search the workspace",
				type: "thread.send_message",
				working_directory: "C:/workspace",
			},
			protocol_version: 1,
			schema_version: 1,
			sent_at: "2026-07-16T10:00:00.000Z",
			thread_id,
		};
		const accepted = yield* repository.Accept(command, false);

		yield* repository.MarkRunStarted(accepted.run_id);

		return accepted.run_id;
	});

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("orchestration capability projection", () => {
	it("projects stable opaque capability identities without publishing provider-controlled metadata", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			migrations_path,
		});

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SetupThread("thread_1");

					const run_id = yield* OpenRun("thread_1");
					const repository = yield* OrchestrationRepository;
					const tool_started: EngineObservation = {
						_tag: "tool",
						action: "started",
						artisan_run_id: run_id,
						detail: "secret_tool_detail",
						observation_id: "secret_observation_started",
						raw: {
							engine_id: "token=secret_engine",
							frame: {
								arguments: { token: "secret_tool_argument" },
							},
							native_id: "secret_native_started",
							transport: "stdio-jsonl",
						},
						sequence: 7,
						tool_id: "token=secret_tool_id",
						tool_name: "Search C:/secret/customer-data",
					};
					const tool_completed: EngineObservation = {
						...tool_started,
						action: "completed",
						observation_id: "secret_observation_completed",
						raw: {
							...tool_started.raw,
							engine_id: "token=changed_raw_engine",
							frame: { result: "secret_tool_result" },
							native_id: "secret_native_completed",
						},
						sequence: 8,
					};
					const native_action_first: EngineObservation = {
						_tag: "native_action",
						action: "secret_native_action_first",
						artisan_run_id: run_id,
						detail: "secret_native_detail",
						observation_id: "secret_observation_native_first",
						raw: {
							engine_id: "token=secret_engine",
							frame: {
								arguments: { token: "secret_native_argument" },
								result: "secret_native_result",
							},
							native_id: "secret_native_action_id_first",
							transport: "stdio-jsonl",
						},
						sequence: 9,
					};
					const native_action_second: EngineObservation = {
						...native_action_first,
						action: "secret_native_action_second",
						observation_id: "secret_observation_native_second",
						raw: {
							...native_action_first.raw,
							engine_id: "token=another_raw_engine",
							native_id: "secret_native_action_id_second",
						},
					};
					const started_events = yield* repository.RecordObservation(tool_started);
					const completed_events = yield* repository.RecordObservation(tool_completed);
					const first_native_events =
						yield* repository.RecordObservation(native_action_first);
					const second_native_events =
						yield* repository.RecordObservation(native_action_second);
					const duplicate_events = yield* repository.RecordObservation(tool_started);

					yield* SetupThread("thread_2");

					const second_run_id = yield* OpenRun("thread_2", "engine_2");
					const cross_run_events = yield* repository.RecordObservation({
						...tool_started,
						artisan_run_id: second_run_id,
					});
					const database = yield* Database;
					const raw_observations = yield* database.client
						.select()
						.from(OrchestrationRawObservations);
					const projected_events = yield* database.client
						.select()
						.from(JournalEvents)
						.pipe(
							Effect.map((events) =>
								events.filter((event) => event.run_id === run_id),
							),
						);

					return {
						completed_events,
						cross_run_events,
						duplicate_events,
						first_native_events,
						projected_events,
						raw_observations,
						second_native_events,
						started_events,
					};
				}),
			);

			const events = [
				...result.started_events,
				...result.completed_events,
				...result.first_native_events,
				...result.second_native_events,
			];
			const projected = result.projected_events.filter((event) =>
				["capability.invocation.updated", "engine.native_action.observed"].includes(
					event.event_type,
				),
			);

			expect(result.duplicate_events).toEqual([]);
			expect(result.raw_observations).toHaveLength(5);
			expect(JSON.stringify(result.raw_observations)).toContain("secret_tool_result");
			expect(
				result.raw_observations.filter(
					(observation) => observation.observation_id === "secret_observation_started",
				),
			).toHaveLength(2);
			expect(events.map((event) => event.payload.type)).toEqual([
				"capability.invocation.updated",
				"capability.invocation.updated",
				"engine.native_action.observed",
				"engine.native_action.observed",
			]);
			expect(events.map((event) => event.journal_sequence)).toEqual(
				[...events]
					.map((event) => event.journal_sequence)
					.sort((left, right) => left - right),
			);
			const tool_events = events.filter(is_capability_event);
			const native_events = events.filter(is_native_action_event);
			const public_json = JSON.stringify({ events, projected });

			expect(tool_events.map((event) => event.payload.state)).toEqual([
				"started",
				"completed",
			]);
			expect(tool_events[0]!.payload.invocation_id).toBe(
				tool_events[1]!.payload.invocation_id,
			);
			expect(tool_events[0]!.payload.invocation_id).toMatch(/^engine_tool:[a-f0-9]{64}$/u);
			expect(result.cross_run_events).toHaveLength(1);

			const cross_run_event = result.cross_run_events[0]!;

			expect(cross_run_event.payload).toMatchObject({
				label: "Engine tool",
				state: "started",
				type: "capability.invocation.updated",
			});

			if (!is_capability_event(cross_run_event)) {
				throw new Error("Expected a canonical capability event");
			}

			expect(cross_run_event.payload.invocation_id).not.toBe(
				tool_events[0]!.payload.invocation_id,
			);
			expect(cross_run_event.raw_origin?.provider).not.toBe(events[0]!.raw_origin?.provider);
			expect(native_events[0]!.payload.action_id).not.toBe(
				native_events[1]!.payload.action_id,
			);
			expect(events.map((event) => event.causation_id)).toHaveLength(
				new Set(events.map((event) => event.causation_id)).size,
			);
			expect(events.map((event) => event.raw_origin?.provider)).toEqual([
				events[0]!.raw_origin?.provider,
				events[0]!.raw_origin?.provider,
				events[0]!.raw_origin?.provider,
				events[0]!.raw_origin?.provider,
			]);
			expect(events[0]!.raw_origin?.provider).toMatch(/^engine:[a-f0-9]{64}$/u);
			expect(events.map((event) => event.raw_origin?.reference)).toHaveLength(
				new Set(events.map((event) => event.raw_origin?.reference)).size,
			);
			expect(projected).toHaveLength(4);
			expect(public_json).not.toContain("secret");
			expect(JSON.stringify(projected)).not.toContain("secret_");
		} finally {
			await runtime.dispose();
		}
	});
});
