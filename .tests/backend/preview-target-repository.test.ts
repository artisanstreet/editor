import { fileURLToPath } from "node:url";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, FileSystem, Layer, ManagedRuntime, Option, Schema } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	CommandEnvelope,
	PreviewTargetUpdatedEvent,
	RawOrigin,
	type CommandEnvelope as Command,
} from "@artisan/protocol";

import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import {
	EventStreams,
	JournalCommands,
	JournalEvents,
	PreviewTargetRemovalFences,
	PreviewTargets,
	Threads,
} from "../../modules/backend/src/persistence/schema";
import { RuntimeMetadataLive } from "../../modules/backend/src/runtime/runtime-metadata";
import {
	PreviewBrowserRepository,
	PreviewBrowserRepositoryLive,
} from "../../modules/backend/src/preview/preview-browser-repository";
import type { PreviewTargetRemovalClaim } from "../../modules/backend/src/preview/preview-browser";
import {
	PreviewTargetRepository,
	PreviewTargetRepositoryLive,
} from "../../modules/backend/src/preview/preview-target-repository";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const runtimes: Array<{ readonly dispose: () => Promise<void> }> = [];
const paths: Array<string> = [];
const now = "2026-07-14T22:00:00.000Z";
type RegisterPayload = Extract<Command["payload"], { readonly type: "preview.target.register" }>;
type RegisterCommand = Omit<Command, "payload"> & { readonly payload: RegisterPayload };
type RemovePayload = Extract<Command["payload"], { readonly type: "preview.target.remove" }>;
type RemoveCommand = Omit<Command, "payload"> & { readonly payload: RemovePayload };

const DecodeCommandPayloadJson = Schema.decodeUnknownEffect(
	Schema.fromJsonString(CommandEnvelope.fields.payload),
	{ onExcessProperty: "error" },
);
const DecodeRawOriginJson = Schema.decodeUnknownEffect(Schema.fromJsonString(RawOrigin), {
	onExcessProperty: "error",
});
const DecodePreviewEventJson = Schema.decodeUnknownEffect(
	Schema.fromJsonString(PreviewTargetUpdatedEvent),
	{ onExcessProperty: "error" },
);

interface RegisterOptions {
	readonly agent_id?: string;
	readonly causation_id?: string;
	readonly message_id?: string;
	readonly project_id?: string;
	readonly raw_origin?: { readonly provider: string; readonly reference: string };
	readonly run_id?: string;
	readonly sent_at?: string;
	readonly target_id?: string;
	readonly thread_id?: string;
	readonly url?: string;
	readonly workspace_id?: string;
}

function register_command(options: RegisterOptions = {}): RegisterCommand {
	return {
		...(options.agent_id === undefined ? {} : { agent_id: options.agent_id }),
		...(options.causation_id === undefined ? {} : { causation_id: options.causation_id }),
		kind: "command",
		message_id: options.message_id ?? "register_1",
		origin: "frontend",
		payload: {
			project_id: options.project_id ?? "project_1",
			target_id: options.target_id ?? "preview_1",
			type: "preview.target.register",
			url: options.url ?? "http://localhost:5173/",
			workspace_id: options.workspace_id ?? "workspace_1",
		},
		protocol_version: 1,
		...(options.raw_origin === undefined ? {} : { raw_origin: options.raw_origin }),
		...(options.run_id === undefined ? {} : { run_id: options.run_id }),
		schema_version: 1,
		sent_at: options.sent_at ?? now,
		thread_id: options.thread_id ?? "thread_1",
	};
}

function remove_command(options: Omit<RegisterOptions, "url"> = {}): RemoveCommand {
	return {
		kind: "command",
		message_id: options.message_id ?? "remove_1",
		origin: "frontend",
		payload: {
			project_id: options.project_id ?? "project_1",
			target_id: options.target_id ?? "preview_1",
			type: "preview.target.remove",
			workspace_id: options.workspace_id ?? "workspace_1",
		},
		protocol_version: 1,
		schema_version: 1,
		sent_at: options.sent_at ?? "2026-07-14T22:00:01.000Z",
		thread_id: options.thread_id ?? "thread_1",
	};
}

function make_seed_targets(length: number) {
	return Array.from({ length }, (_, index) => ({
		created_at_ms: 1,
		generation_id: `preview_target_seed_${index}`,
		project_id: "project_1",
		state: "registered",
		target_id: `preview_${index}`,
		updated_at_ms: 1,
		url: "http://localhost:5173/",
		workspace_id: "workspace_1",
	}));
}

const MakeDatabasePath = Effect.flatMap(FileSystem.FileSystem, (file_system) =>
	file_system.makeTempDirectory({ prefix: "artisan-preview-repository-" }).pipe(
		Effect.tap((path) => Effect.sync(() => paths.push(path))),
		Effect.map((path) => `${path}/artisan.db`),
	),
);

function make_runtime(database_path: string) {
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		JournalNotifierLive,
		RuntimeMetadataLive,
	);
	const repository = PreviewTargetRepositoryLive.pipe(Layer.provide(infrastructure));
	const browser_repository = PreviewBrowserRepositoryLive.pipe(Layer.provide(infrastructure));
	const runtime = ManagedRuntime.make(
		Layer.mergeAll(repository, browser_repository, infrastructure),
	);

	runtimes.push(runtime);

	return runtime;
}

const RemoveClaimed = (command: RemoveCommand, now_ms: number) =>
	Effect.gen(function* () {
		const browser_repository = yield* PreviewBrowserRepository;
		const repository = yield* PreviewTargetRepository;
		const claim = yield* browser_repository
			.ClaimTargetRemoval(
				{
					project_id: command.payload.project_id,
					target_id: command.payload.target_id,
					workspace_id: command.payload.workspace_id,
				},
				now_ms,
				10_000,
			)
			.pipe(Effect.orDie);

		return yield* repository
			.RemoveClaimed(command, claim, now_ms)
			.pipe(
				Effect.ensuring(browser_repository.ReleaseTargetRemoval(claim).pipe(Effect.orDie)),
			);
	});

async function dispose_runtime(runtime: { readonly dispose: () => Promise<void> }) {
	const index = runtimes.indexOf(runtime);

	if (index >= 0) {
		runtimes.splice(index, 1);
	}

	await runtime.dispose();
}

afterEach(async () => {
	await Promise.all(runtimes.splice(0).map((runtime) => runtime.dispose()));
	await Effect.runPromise(
		Effect.forEach(
			paths.splice(0),
			(path) =>
				Effect.flatMap(FileSystem.FileSystem, (file_system) =>
					file_system.remove(path, { recursive: true }),
				),
			{ discard: true },
		).pipe(Effect.provide(NodeFileSystem.layer)),
	);
});

describe("PreviewTargetRepository", () => {
	it("persists canonical attribution and exact replay across a runtime restart", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const first = make_runtime(database_path);
		const command = register_command({
			agent_id: "agent_1",
			causation_id: "source_1",
			raw_origin: { provider: "codex", reference: "native_1" },
			run_id: "run_1",
		});
		const accepted = await first.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;
				const repository = yield* PreviewTargetRepository;

				yield* database.client.insert(Threads).values({
					created_at: now,
					thread_id: "thread_1",
					title: "Preview thread",
					title_source: "initial",
					updated_at: now,
				});
				yield* database.client.insert(EventStreams).values({
					last_sequence: 7,
					stream_id: "thread:thread_1",
				});

				return yield* repository.Register(command, command.payload.url, 10_000);
			}),
		);

		await dispose_runtime(first);

		const second = make_runtime(database_path);
		const result = await second.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;
				const repository = yield* PreviewTargetRepository;
				const replayed = yield* repository.Register(command, command.payload.url, 20_000);
				const commands = yield* database.client.select().from(JournalCommands);
				const events = yield* database.client.select().from(JournalEvents);
				const streams = yield* database.client.select().from(EventStreams);
				const targets = yield* database.client.select().from(PreviewTargets);

				return { commands, events, replayed, streams, targets };
			}),
		);
		const stored_command = result.commands[0];
		const stored_event = result.events[0];

		if (stored_command === undefined || stored_event === undefined) {
			throw new Error("expected persisted preview command and event");
		}

		const decoded_command = await Effect.runPromise(
			DecodeCommandPayloadJson(stored_command.payload_json),
		);
		const decoded_raw_origin = await Effect.runPromise(
			DecodeRawOriginJson(stored_event.raw_origin_json ?? ""),
		);
		const decoded_event = await Effect.runPromise(
			DecodePreviewEventJson(stored_event.payload_json),
		);

		expect(accepted.status).toBe("accepted");
		expect(result.replayed).toEqual({ event: accepted.event, status: "duplicate" });
		expect(result.replayed.event).toMatchObject({
			agent_id: "agent_1",
			raw_origin: { provider: "codex", reference: "native_1" },
			run_id: "run_1",
		});
		expect(result.commands).toHaveLength(1);
		expect(result.events).toHaveLength(1);
		expect(result.targets).toHaveLength(1);
		expect(result.streams).toEqual([{ last_sequence: 8, stream_id: "thread:thread_1" }]);
		expect(decoded_command).toEqual(command.payload);
		expect(decoded_raw_origin).toEqual(command.raw_origin);
		expect(decoded_event).toEqual(accepted.event.payload);
	});

	it("rejects partial source and health projections at the SQLite boundary", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const runtime = make_runtime(database_path);
		const result = await runtime.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;
				const source = yield* database.client
					.insert(PreviewTargets)
					.values({
						created_at_ms: 1,
						generation_id: "preview_target_partial_source",
						project_id: "project_1",
						source_id: "process_1",
						source_kind: null,
						state: "registered",
						target_id: "partial_source",
						updated_at_ms: 1,
						url: "http://localhost:5173/",
						workspace_id: "workspace_1",
					})
					.pipe(Effect.result);
				const health = yield* database.client
					.insert(PreviewTargets)
					.values({
						created_at_ms: 1,
						generation_id: "preview_target_partial_health",
						health_message: "partial health",
						health_status: null,
						project_id: "project_1",
						state: "registered",
						target_id: "partial_health",
						updated_at_ms: 1,
						url: "http://localhost:5173/",
						workspace_id: "workspace_1",
					})
					.pipe(Effect.result);
				const rows = yield* database.client.select().from(PreviewTargets);

				return { health, rows, source };
			}),
		);

		expect(result.source._tag).toBe("Failure");
		expect(result.health._tag).toBe("Failure");
		expect(result.rows).toEqual([]);
	});

	it("keeps identical target ids isolated by project and workspace", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const runtime = make_runtime(database_path);
		const result = await runtime.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;
				const repository = yield* PreviewTargetRepository;

				yield* database.client.insert(Threads).values({
					created_at: now,
					thread_id: "thread_1",
					title: "Preview thread",
					title_source: "initial",
					updated_at: now,
				});
				yield* database.client.insert(EventStreams).values({
					last_sequence: 0,
					stream_id: "thread:thread_1",
				});

				const first = register_command({ message_id: "register_scope_1" });
				const second = register_command({
					message_id: "register_scope_2",
					project_id: "project_2",
					workspace_id: "workspace_2",
				});

				yield* repository.Register(first, first.payload.url, 1);
				yield* repository.Register(second, second.payload.url, 2);
				const cross_scope_remove = yield* RemoveClaimed(
					remove_command({
						message_id: "remove_cross_scope",
						workspace_id: "workspace_2",
					}),
					3,
				).pipe(Effect.flip);

				const removed = yield* RemoveClaimed(
					remove_command({ message_id: "remove_scope_1" }),
					4,
				);

				const first_scope = yield* repository.List({
					project_id: "project_1",
					workspace_id: "workspace_1",
				});
				const second_scope = yield* repository.List({
					project_id: "project_2",
					workspace_id: "workspace_2",
				});
				const cross_scope = yield* repository.Get({
					project_id: "project_1",
					target_id: "preview_1",
					workspace_id: "workspace_2",
				});

				return { cross_scope, cross_scope_remove, first_scope, removed, second_scope };
			}),
		);

		expect(result.cross_scope_remove).toMatchObject({ reason: "target" });
		expect(result.first_scope).toEqual([]);
		expect(result.removed.event.payload).toMatchObject({
			action: "removed",
			target: {
				project_id: "project_1",
				state: "removed",
				target_id: "preview_1",
				workspace_id: "workspace_1",
			},
			type: "preview.target.updated",
		});
		expect(result.second_scope).toMatchObject([
			{ project_id: "project_2", target_id: "preview_1", workspace_id: "workspace_2" },
		]);
		expect(Option.isNone(result.cross_scope)).toBe(true);
	});

	it("binds a target-removal claim to its exact target generation", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const runtime = make_runtime(database_path);
		const result = await runtime.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;
				const browser_repository = yield* PreviewBrowserRepository;
				const repository = yield* PreviewTargetRepository;

				yield* database.client.insert(Threads).values({
					created_at: now,
					thread_id: "thread_1",
					title: "Preview thread",
					title_source: "initial",
					updated_at: now,
				});
				yield* database.client.insert(EventStreams).values({
					last_sequence: 0,
					stream_id: "thread:thread_1",
				});

				const registration = register_command();
				const command = remove_command();

				yield* repository.Register(registration, registration.payload.url, 1);
				const claim = yield* browser_repository.ClaimTargetRemoval(
					{
						project_id: command.payload.project_id,
						target_id: command.payload.target_id,
						workspace_id: command.payload.workspace_id,
					},
					2,
					10_000,
				);
				const forged_claim = {
					...claim,
					subject: {
						_tag: "Current",
						target_generation_id: "forged_generation",
					},
				} satisfies PreviewTargetRemovalClaim;

				const removal_error = yield* repository
					.RemoveClaimed(command, forged_claim, 3)
					.pipe(Effect.flip);
				const browser_error = yield* browser_repository
					.ActiveInspectionIdsForTargetRemoval(forged_claim, 3)
					.pipe(Effect.flip);
				const targets = yield* database.client.select().from(PreviewTargets);
				const fences = yield* database.client.select().from(PreviewTargetRemovalFences);

				yield* browser_repository.ReleaseTargetRemoval(claim);

				return { browser_error, fences, removal_error, targets };
			}),
		);

		expect(result.removal_error).toMatchObject({ reason: "target_removing" });
		expect(result.browser_error).toMatchObject({ reason: "ownership_lost" });
		expect(result.targets).toHaveLength(1);
		expect(result.fences).toEqual([]);
	});

	it("rejects changed intent and attribution for one command identity", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const runtime = make_runtime(database_path);
		const errors = await runtime.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;
				const repository = yield* PreviewTargetRepository;

				yield* database.client.insert(Threads).values({
					created_at: now,
					thread_id: "thread_1",
					title: "Preview thread",
					title_source: "initial",
					updated_at: now,
				});
				yield* database.client.insert(EventStreams).values({
					last_sequence: 0,
					stream_id: "thread:thread_1",
				});

				const original = register_command();

				yield* repository.Register(original, original.payload.url, 1);

				const changed_attribution = yield* repository
					.Register(register_command({ agent_id: "agent_2" }), original.payload.url, 2)
					.pipe(Effect.flip);
				const changed_intent = yield* repository
					.Register(
						register_command({ url: "http://localhost:4173/" }),
						"http://localhost:4173/",
						3,
					)
					.pipe(Effect.flip);

				return { changed_attribution, changed_intent };
			}),
		);

		expect(errors.changed_attribution).toMatchObject({ reason: "command_conflict" });
		expect(errors.changed_intent).toMatchObject({ reason: "command_conflict" });
	});

	it("serializes one command across concurrent runtimes", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const first = make_runtime(database_path);

		await first.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;

				yield* database.client.insert(Threads).values({
					created_at: now,
					thread_id: "thread_1",
					title: "Preview thread",
					title_source: "initial",
					updated_at: now,
				});
				yield* database.client.insert(EventStreams).values({
					last_sequence: 0,
					stream_id: "thread:thread_1",
				});
			}),
		);

		const second = make_runtime(database_path);
		const command = register_command({ message_id: "concurrent_register" });
		const accept = (runtime: typeof first) =>
			runtime.runPromise(
				Effect.flatMap(PreviewTargetRepository, (repository) =>
					repository.Register(command, command.payload.url, 1),
				),
			);
		const results = await Promise.all([accept(first), accept(second)]);
		const rows = await first.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;

				return {
					commands: yield* database.client.select().from(JournalCommands),
					events: yield* database.client.select().from(JournalEvents),
					streams: yield* database.client.select().from(EventStreams),
				};
			}),
		);

		expect(results.map(({ status }) => status).toSorted()).toEqual(["accepted", "duplicate"]);
		expect(rows.commands).toHaveLength(1);
		expect(rows.events).toHaveLength(1);
		expect(rows.streams).toEqual([{ last_sequence: 1, stream_id: "thread:thread_1" }]);
	});

	it("serializes distinct concurrent commands onto one thread stream", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const first = make_runtime(database_path);

		await first.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;

				yield* database.client.insert(Threads).values({
					created_at: now,
					thread_id: "thread_1",
					title: "Preview thread",
					title_source: "initial",
					updated_at: now,
				});
				yield* database.client.insert(EventStreams).values({
					last_sequence: 0,
					stream_id: "thread:thread_1",
				});
			}),
		);

		const second = make_runtime(database_path);
		const first_command = register_command({
			message_id: "concurrent_distinct_1",
			target_id: "preview_1",
		});
		const second_command = register_command({
			message_id: "concurrent_distinct_2",
			target_id: "preview_2",
		});
		const accept = (runtime: typeof first, command: RegisterCommand) =>
			runtime.runPromise(
				Effect.flatMap(PreviewTargetRepository, (repository) =>
					repository.Register(command, command.payload.url, 1),
				),
			);
		const results = await Promise.all([
			accept(first, first_command),
			accept(second, second_command),
		]);
		const rows = await first.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;

				return {
					commands: yield* database.client.select().from(JournalCommands),
					events: yield* database.client.select().from(JournalEvents),
					streams: yield* database.client.select().from(EventStreams),
					targets: yield* database.client.select().from(PreviewTargets),
				};
			}),
		);

		expect(results.map(({ status }) => status)).toEqual(["accepted", "accepted"]);
		expect(rows.commands).toHaveLength(2);
		expect(rows.events.map(({ stream_sequence }) => stream_sequence).toSorted()).toEqual([
			1, 2,
		]);
		expect(rows.streams).toEqual([{ last_sequence: 2, stream_id: "thread:thread_1" }]);
		expect(rows.targets).toHaveLength(2);
	});

	it("rejects one of two concurrent intents sharing a command identity", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const first = make_runtime(database_path);

		await first.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;

				yield* database.client.insert(Threads).values({
					created_at: now,
					thread_id: "thread_1",
					title: "Preview thread",
					title_source: "initial",
					updated_at: now,
				});
				yield* database.client.insert(EventStreams).values({
					last_sequence: 0,
					stream_id: "thread:thread_1",
				});
			}),
		);

		const second = make_runtime(database_path);
		const first_command = register_command({
			message_id: "concurrent_conflict",
			url: "http://localhost:5173/",
		});
		const second_command = register_command({
			message_id: "concurrent_conflict",
			url: "http://localhost:4173/",
		});
		const accept = (runtime: typeof first, command: RegisterCommand) =>
			runtime.runPromise(
				Effect.flatMap(PreviewTargetRepository, (repository) =>
					repository.Register(command, command.payload.url, 1),
				).pipe(Effect.result),
			);
		const results = await Promise.all([
			accept(first, first_command),
			accept(second, second_command),
		]);
		const outcomes = results.map((result) =>
			result._tag === "Failure" ? result.failure._tag : result.success.status,
		);
		const rows = await first.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;

				return {
					commands: yield* database.client.select().from(JournalCommands),
					events: yield* database.client.select().from(JournalEvents),
					streams: yield* database.client.select().from(EventStreams),
					targets: yield* database.client.select().from(PreviewTargets),
				};
			}),
		);

		expect(outcomes.toSorted()).toEqual(["PreviewTargetRepositoryConflict", "accepted"]);
		expect(rows.commands).toHaveLength(1);
		expect(rows.events).toHaveLength(1);
		expect(rows.streams).toEqual([{ last_sequence: 1, stream_id: "thread:thread_1" }]);
		expect(rows.targets).toHaveLength(1);
		expect([first_command.payload.url, second_command.payload.url]).toContain(
			rows.targets[0]?.url,
		);
	});

	it("rejects a command for a nonexistent thread without creating a stream", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const runtime = make_runtime(database_path);
		const result = await runtime.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;
				const repository = yield* PreviewTargetRepository;
				const command = register_command({ thread_id: "missing_thread" });
				const error = yield* repository
					.Register(command, command.payload.url, 1)
					.pipe(Effect.flip);

				return {
					commands: yield* database.client.select().from(JournalCommands),
					error,
					events: yield* database.client.select().from(JournalEvents),
					streams: yield* database.client.select().from(EventStreams),
				};
			}),
		);

		expect(result.error).toMatchObject({ reason: "thread" });
		expect(result.commands).toEqual([]);
		expect(result.events).toEqual([]);
		expect(result.streams).toEqual([]);
	});

	it("rejects a target that would exceed the bounded public projection", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const runtime = make_runtime(database_path);
		const result = await runtime.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;
				const repository = yield* PreviewTargetRepository;
				const targets = make_seed_targets(256);

				yield* database.client.insert(Threads).values({
					created_at: now,
					thread_id: "thread_1",
					title: "Preview thread",
					title_source: "initial",
					updated_at: now,
				});
				yield* database.client.insert(EventStreams).values({
					last_sequence: 0,
					stream_id: "thread:thread_1",
				});
				yield* database.client.insert(PreviewTargets).values(targets);

				const command = register_command({
					message_id: "register_over_limit",
					target_id: "preview_over_limit",
				});
				const error = yield* repository
					.Register(command, command.payload.url, 2)
					.pipe(Effect.flip);

				return {
					commands: yield* database.client.select().from(JournalCommands),
					error,
					events: yield* database.client.select().from(JournalEvents),
					streams: yield* database.client.select().from(EventStreams),
					targets: yield* database.client.select().from(PreviewTargets),
				};
			}),
		);

		expect(result.error).toMatchObject({ reason: "target_limit" });
		expect(result.commands).toEqual([]);
		expect(result.events).toEqual([]);
		expect(result.streams).toEqual([{ last_sequence: 0, stream_id: "thread:thread_1" }]);
		expect(result.targets).toHaveLength(256);
	});

	it("serializes concurrent registrations at the public projection limit", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const first = make_runtime(database_path);

		await first.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;

				yield* database.client.insert(Threads).values({
					created_at: now,
					thread_id: "thread_1",
					title: "Preview thread",
					title_source: "initial",
					updated_at: now,
				});
				yield* database.client.insert(EventStreams).values({
					last_sequence: 0,
					stream_id: "thread:thread_1",
				});
				yield* database.client.insert(PreviewTargets).values(make_seed_targets(255));
			}),
		);

		const second = make_runtime(database_path);
		const first_command = register_command({
			message_id: "register_limit_a",
			target_id: "preview_limit_a",
		});
		const second_command = register_command({
			message_id: "register_limit_b",
			target_id: "preview_limit_b",
		});
		const accept = (runtime: typeof first, command: RegisterCommand) =>
			runtime.runPromise(
				Effect.flatMap(PreviewTargetRepository, (repository) =>
					repository.Register(command, command.payload.url, 2).pipe(Effect.result),
				),
			);
		const results = await Promise.all([
			accept(first, first_command),
			accept(second, second_command),
		]);
		const rows = await first.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;

				return {
					commands: yield* database.client.select().from(JournalCommands),
					events: yield* database.client.select().from(JournalEvents),
					streams: yield* database.client.select().from(EventStreams),
					targets: yield* database.client.select().from(PreviewTargets),
				};
			}),
		);
		const outcomes = results.map((result) =>
			result._tag === "Failure" ? result.failure : result.success.status,
		);

		expect(outcomes).toContain("accepted");
		expect(outcomes).toContainEqual(
			expect.objectContaining({
				_tag: "PreviewTargetRepositoryConflict",
				reason: "target_limit",
			}),
		);
		expect(rows.commands).toHaveLength(1);
		expect(rows.events).toHaveLength(1);
		expect(rows.streams).toEqual([{ last_sequence: 1, stream_id: "thread:thread_1" }]);
		expect(rows.targets).toHaveLength(256);
	});
});
