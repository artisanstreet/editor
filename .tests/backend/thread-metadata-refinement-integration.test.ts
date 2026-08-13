import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Layer } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { CommandEnvelope, ProjectRef } from "@artisan/protocol";
import { make_backend_runtime, ProtocolRouter } from "@artisan/backend";

import { Database } from "../../modules/backend/src/persistence/database";
import { JournalCommands, JournalEvents } from "../../modules/backend/src/persistence/tables";
import { ThreadReadModel } from "../../modules/backend/src/persistence/thread-read-model";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/metadata";
import { ThreadMetadataRepository } from "../../modules/backend/src/threads/thread-metadata-repository";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];

const ProjectArtisan: ProjectRef = {
	display_name: "Artisan Editor",
	project_id: "project_artisan",
	root_path: "C:/work/artisan-editor",
};

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-metadata-refinement-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

function make_metadata_layer() {
	let next_id = 0;

	return Layer.succeed(RuntimeMetadata, {
		instance_id: "metadata_refinement_integration_test",
		MakeId: (prefix) => Effect.sync(() => `${prefix}_${++next_id}`),
		Now: Effect.succeed("2026-07-11T13:00:00.000Z"),
	});
}

function make_command(message_id: string, payload: CommandEnvelope["payload"]): CommandEnvelope {
	return {
		kind: "command",
		message_id,
		origin: "frontend",
		payload,
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-11T13:00:00.000Z",
		thread_id: "thread_refinement",
	};
}

function make_refinement(
	operation_id: string,
	source_event_id = "source_message_1",
	mentioned_projects?: ReadonlyArray<ProjectRef>,
) {
	return {
		operation_id,
		payload: {
			basis_activity_version: 0,
			basis_metadata_version: 0,
			current_goal: "Ship automatic thread metadata",
			...(mentioned_projects === undefined ? {} : { mentioned_projects }),
			rename_suggestion: "Automatic metadata refinement",
			title: "Build automatic metadata refinement",
			type: "thread.metadata.refine" as const,
		},
		source_event_id,
		thread_id: "thread_refinement",
	};
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("automatic thread metadata refinement integration", () => {
	it("journals a backend-owned refinement with source lineage and deduplicates by source", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			runtime_metadata: make_metadata_layer(),
		});

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* ThreadMetadataRepository;
					const router = yield* ProtocolRouter;
					const threads = yield* ThreadReadModel;

					yield* router.Route(
						make_command("create_refinement", {
							title: "Initial request title",
							type: "thread.create",
						}),
					);
					const first = yield* repository.Refine(
						make_refinement("metadata-refine:source_message_1", "source_message_1", [
							ProjectArtisan,
						]),
					);
					const duplicate = yield* repository.Refine({
						...make_refinement("different_operation_id"),
						payload: {
							...make_refinement("different_operation_id").payload,
							title: "A nondeterministic replay must not win",
						},
					});

					return {
						commands: yield* database.client.select().from(JournalCommands),
						duplicate,
						events: yield* database.client.select().from(JournalEvents),
						first,
						thread: (yield* threads.Snapshot()).threads[0]!,
						was_refined: yield* repository.WasRefined(
							"source_message_1",
							"thread_refinement",
						),
					};
				}),
			);
			const refinement_commands = result.commands.filter(
				(command) => command.payload_type === "thread.metadata.refine",
			);
			const [refinement_event] = result.events.filter(
				(event) => event.event_type === "thread.metadata.updated",
			);

			expect(result.first.status).toBe("accepted");
			expect(result.duplicate.status).toBe("duplicate");
			expect(result.duplicate.event).toEqual(result.first.event);
			expect(result.was_refined).toBe(true);
			expect(refinement_commands).toHaveLength(1);
			expect(refinement_commands[0]).toMatchObject({
				causation_id: "source_message_1",
				message_id: "metadata-refine:source_message_1",
				origin: "backend",
			});
			expect(refinement_event).toMatchObject({
				causation_id: "source_message_1",
				correlation_id: "metadata-refine:source_message_1",
				origin: "backend",
			});
			expect(JSON.parse(refinement_event!.payload_json)).toMatchObject({
				mentioned_projects: [ProjectArtisan],
				type: "thread.metadata.updated",
			});
			expect(result.thread).toMatchObject({
				current_goal: "Ship automatic thread metadata",
				metadata_version: 1,
				title: "Build automatic metadata refinement",
				title_source: "automatic",
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("records a stale source refinement once without changing the projection", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			runtime_metadata: make_metadata_layer(),
		});

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* ThreadMetadataRepository;
					const router = yield* ProtocolRouter;
					const threads = yield* ThreadReadModel;

					yield* router.Route(
						make_command("create_stale_refinement", {
							title: "Initial title",
							type: "thread.create",
						}),
					);
					yield* router.Route(
						make_command("activity_before_refinement", {
							activity_kind: "user_message",
							type: "thread.activity.record",
						}),
					);
					const first = yield* repository.Refine(
						make_refinement("metadata-refine:source_stale", "source_stale"),
					);
					const duplicate = yield* repository.Refine(
						make_refinement("metadata-refine:source_stale", "source_stale"),
					);

					return {
						duplicate,
						first,
						thread: (yield* threads.Snapshot()).threads[0]!,
					};
				}),
			);

			expect(result.first.event.payload).toMatchObject({
				basis_activity_version: 0,
				basis_metadata_version: 0,
				type: "thread.refinement.ignored",
			});
			expect(result.duplicate.status).toBe("duplicate");
			expect(result.thread).toMatchObject({
				activity_version: 1,
				metadata_version: 0,
				title: "Initial title",
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("serializes concurrent refinements for one source into one command and event", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			runtime_metadata: make_metadata_layer(),
		});

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* ThreadMetadataRepository;
					const router = yield* ProtocolRouter;

					yield* router.Route(
						make_command("create_concurrent_refinement", {
							title: "Concurrent title",
							type: "thread.create",
						}),
					);
					const acceptances = yield* Effect.all(
						Array.from({ length: 8 }, (_, index) =>
							repository.Refine(
								make_refinement(
									`metadata-refine:concurrent:${index}`,
									"source_concurrent",
								),
							),
						),
						{ concurrency: "unbounded" },
					);
					const commands = yield* database.client.select().from(JournalCommands);
					const events = yield* database.client.select().from(JournalEvents);

					return {
						acceptances,
						commands: commands.filter(
							(command) => command.payload_type === "thread.metadata.refine",
						),
						events: events.filter(
							(event) => event.event_type === "thread.metadata.updated",
						),
					};
				}),
			);

			expect(result.acceptances.filter(({ status }) => status === "accepted")).toHaveLength(
				1,
			);
			expect(result.acceptances.filter(({ status }) => status === "duplicate")).toHaveLength(
				7,
			);
			expect(result.commands).toHaveLength(1);
			expect(result.events).toHaveLength(1);
		} finally {
			await runtime.dispose();
		}
	});
});
