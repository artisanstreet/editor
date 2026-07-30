import { createHash } from "node:crypto";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { NodeCrypto } from "@effect/platform-node-shared";
import { Deferred, Effect, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { make_backend_runtime } from "@artisan/backend";

import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import {
	EventStreams,
	JournalEvents,
	ThreadErasureClaims,
	Threads,
	ThreadTombstones,
	WorkspaceChangeOperations,
	WorkspaceMutationPayloads,
} from "../../modules/backend/src/persistence/tables";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/metadata";
import { ThreadErasure, ThreadErasureLive } from "../../modules/backend/src/threads/thread-erasure";
import { ThreadResourceQuiescer } from "../../modules/backend/src/threads/thread-resource-quiescer";
import {
	WorkspaceChangeRepository,
	WorkspaceChangeRepositoryLive,
} from "../../modules/backend/src/workspace/changes/repository";
import {
	WorkspaceMutationPayloadStore,
	WorkspaceMutationPayloadStoreLive,
	type WorkspaceMutationPayloadStageInput,
} from "../../modules/backend/src/workspace/mutations/payloads";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const created_at = "2026-07-01T12:00:00.000Z";
const cutoff = "2026-07-08T12:00:00.000Z";
const deleted_at = "2026-07-10T12:00:00.000Z";

type PayloadState = "available" | "consumed" | undefined;
type TransactionProbe = {
	readonly attempt: number;
	readonly continue_transaction: Deferred.Deferred<void>;
	readonly phase: "after" | "before";
	readonly transaction_reached: Deferred.Deferred<void>;
};

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-thread-erasure-mutation-fence-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

function make_metadata_layer() {
	let next_id = 0;

	return Layer.succeed(RuntimeMetadata, {
		instance_id: "thread_erasure_mutation_fence_test",
		MakeId: (prefix) => Effect.sync(() => `${prefix}_${++next_id}`),
		Now: Effect.succeed(deleted_at),
	});
}

function make_test_database_layer(database_path: string, probe?: TransactionProbe) {
	const database_layer = make_database_layer({ database_path, migrations_path });

	if (!probe) {
		return database_layer;
	}

	return Layer.effect(
		Database,
		Effect.gen(function* () {
			const database = yield* Database;
			let callback_count = 0;
			const Transaction: typeof database.client.transaction = (operation, config) =>
				database.client.transaction(
					(transaction) =>
						Effect.suspend(() => {
							callback_count += 1;
							const is_probed_attempt = callback_count === probe.attempt;
							const Before =
								is_probed_attempt && probe.phase === "before"
									? Deferred.succeed(probe.transaction_reached, undefined).pipe(
											Effect.andThen(
												Deferred.await(probe.continue_transaction),
											),
										)
									: Effect.void;

							return Before.pipe(
								Effect.andThen(operation(transaction)),
								Effect.flatMap((result) =>
									is_probed_attempt && probe.phase === "after"
										? Deferred.succeed(
												probe.transaction_reached,
												undefined,
											).pipe(
												Effect.andThen(
													Deferred.await(probe.continue_transaction),
												),
												Effect.as(result),
											)
										: Effect.succeed(result),
								),
							);
						}),
					config,
				);
			const client = new Proxy(database.client, {
				get: (target, property, receiver) =>
					property === "transaction"
						? Transaction
						: Reflect.get(target, property, receiver),
			});

			return { client };
		}),
	).pipe(Layer.provide(database_layer));
}

async function make_transaction_probe(
	attempt: number,
	phase: TransactionProbe["phase"],
): Promise<TransactionProbe> {
	return {
		attempt,
		continue_transaction: await Effect.runPromise(Deferred.make<void>()),
		phase,
		transaction_reached: await Effect.runPromise(Deferred.make<void>()),
	};
}

function within_timeout<A>(promise: Promise<A>, label = "Thread erasure race") {
	return Promise.race([
		promise,
		new Promise<never>((_resolve, reject) =>
			setTimeout(() => reject(new Error(`${label} timed out`)), 5_000),
		),
	]);
}

async function wait_for_transaction(probe: TransactionProbe) {
	await within_timeout(Effect.runPromise(Deferred.await(probe.transaction_reached)));
}

async function continue_transaction(probe: TransactionProbe) {
	await Effect.runPromise(Deferred.succeed(probe.continue_transaction, undefined));
}

function make_runtime(
	database_path: string,
	thread_resource_quiescer?: Layer.Layer<ThreadResourceQuiescer>,
) {
	return make_backend_runtime({
		database_path,
		migrations_path,
		runtime_metadata: make_metadata_layer(),
		...(thread_resource_quiescer === undefined ? {} : { thread_resource_quiescer }),
	});
}

function make_payload_runtime(database_path: string, probe?: TransactionProbe) {
	const infrastructure = Layer.mergeAll(
		make_test_database_layer(database_path, probe),
		make_metadata_layer(),
		NodeCrypto.layer,
	);

	return ManagedRuntime.make(
		WorkspaceMutationPayloadStoreLive.pipe(Layer.provideMerge(infrastructure)),
	);
}

function make_erasure_runtime(
	database_path: string,
	options: {
		readonly probe?: TransactionProbe;
		readonly quiescer?: Layer.Layer<ThreadResourceQuiescer>;
	} = {},
) {
	const quiescer =
		options.quiescer ?? Layer.succeed(ThreadResourceQuiescer, { Quiesce: () => Effect.void });
	const infrastructure = Layer.mergeAll(
		make_test_database_layer(database_path, options.probe),
		JournalNotifierLive,
		quiescer,
	);

	return ManagedRuntime.make(ThreadErasureLive.pipe(Layer.provideMerge(infrastructure)));
}

function make_workspace_change_runtime(database_path: string, probe?: TransactionProbe) {
	const infrastructure = Layer.mergeAll(
		make_test_database_layer(database_path, probe),
		make_metadata_layer(),
		JournalNotifierLive,
	);

	return ManagedRuntime.make(
		WorkspaceChangeRepositoryLive.pipe(
			Layer.provideMerge(NodeCrypto.layer),
			Layer.provideMerge(infrastructure),
		),
	);
}

function bytes(value: string) {
	return new TextEncoder().encode(value);
}

function identity(content: Uint8Array) {
	return {
		algorithm: "sha256" as const,
		byte_count: content.byteLength,
		content_hash: createHash("sha256").update(content).digest("hex"),
	};
}

function stage_input(thread_id: string, message_id: string): WorkspaceMutationPayloadStageInput {
	const expected = bytes(`before:${message_id}`);
	const replacement = bytes(`after:${message_id}`);

	return {
		action: "replace",
		expected,
		expected_identity: identity(expected),
		message_id,
		replacement,
		replacement_identity: identity(replacement),
		thread_id,
	};
}

function consume_input(input: WorkspaceMutationPayloadStageInput) {
	return {
		action: input.action,
		expected_identity: input.expected_identity,
		message_id: input.message_id,
		replacement_identity: input.replacement_identity,
		thread_id: input.thread_id,
	};
}

function SeedThread(thread_id: string) {
	return Effect.gen(function* () {
		const database = yield* Database;

		yield* database.client.insert(Threads).values({
			created_at,
			last_activity_at: created_at,
			thread_id,
			title: thread_id,
			title_source: "initial",
			updated_at: created_at,
		});
		yield* database.client.insert(EventStreams).values({
			last_sequence: 0,
			stream_id: `thread:${thread_id}`,
		});
	});
}

function SeedOperation(options: {
	readonly action?: "replace" | "review";
	readonly lifecycle: string;
	readonly message_id: string;
	readonly thread_id: string;
}) {
	return Effect.gen(function* () {
		const database = yield* Database;
		const expected = bytes(`before:${options.message_id}`);
		const replacement = bytes(`after:${options.message_id}`);

		yield* database.client.insert(WorkspaceChangeOperations).values({
			action: options.action ?? "replace",
			agent_id: options.action === "review" ? null : `agent_${options.message_id}`,
			change_id: `change_${options.message_id}`,
			created_at,
			expected_identity_json: JSON.stringify(identity(expected)),
			lifecycle: options.lifecycle,
			message_id: options.message_id,
			path: options.action === "review" ? null : `src/${options.message_id}.ts`,
			request_fingerprint: createHash("sha256").update(options.message_id).digest("hex"),
			result_identity_json:
				options.action === "review" ? null : JSON.stringify(identity(replacement)),
			run_id: options.action === "review" ? null : `run_${options.message_id}`,
			sent_at: created_at,
			thread_id: options.thread_id,
			updated_at: created_at,
			workspace_id: options.action === "review" ? null : `workspace_${options.message_id}`,
		});
	});
}

function SeedPayload(
	thread_id: string,
	message_id: string,
	state: Exclude<PayloadState, undefined>,
) {
	return Effect.gen(function* () {
		const database = yield* Database;
		const expected = bytes(`before:${message_id}`);
		const replacement = bytes(`after:${message_id}`);

		yield* database.client.insert(WorkspaceMutationPayloads).values(
			state === "available"
				? {
						created_at,
						expected: Buffer.from(expected),
						expected_byte_count: expected.byteLength,
						expected_hash: identity(expected).content_hash,
						message_id,
						replacement: Buffer.from(replacement),
						replacement_byte_count: replacement.byteLength,
						replacement_hash: identity(replacement).content_hash,
						state,
						thread_id,
						updated_at: created_at,
					}
				: {
						created_at,
						expected: null,
						expected_byte_count: null,
						expected_hash: null,
						message_id,
						replacement: null,
						replacement_byte_count: null,
						replacement_hash: null,
						state,
						thread_id,
						updated_at: created_at,
					},
		);
	});
}

function SeedClaim(thread_id: string) {
	return Effect.gen(function* () {
		const database = yield* Database;

		yield* database.client.insert(ThreadErasureClaims).values({
			claimed_at: deleted_at,
			thread_id,
		});
	});
}

function ReadThreadState(thread_id: string) {
	return Effect.gen(function* () {
		const database = yield* Database;

		return yield* Effect.all({
			claims: database.client
				.select()
				.from(ThreadErasureClaims)
				.pipe(Effect.map((rows) => rows.filter((row) => row.thread_id === thread_id))),
			events: database.client
				.select()
				.from(JournalEvents)
				.pipe(Effect.map((rows) => rows.filter((row) => row.thread_id === thread_id))),
			operations: database.client
				.select()
				.from(WorkspaceChangeOperations)
				.pipe(Effect.map((rows) => rows.filter((row) => row.thread_id === thread_id))),
			payloads: database.client
				.select()
				.from(WorkspaceMutationPayloads)
				.pipe(Effect.map((rows) => rows.filter((row) => row.thread_id === thread_id))),
			threads: database.client
				.select()
				.from(Threads)
				.pipe(Effect.map((rows) => rows.filter((row) => row.thread_id === thread_id))),
			tombstones: database.client
				.select()
				.from(ThreadTombstones)
				.pipe(Effect.map((rows) => rows.filter((row) => row.thread_id === thread_id))),
		});
	});
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("ThreadErasure workspace mutation fence", () => {
	it.each([
		["claimed available", "claimed", "available"],
		["applied available", "applied", "available"],
		["applied missing", "applied", undefined],
		["committed available", "committed", "available"],
		["rejected available", "rejected", "available"],
		["claimed consumed", "claimed", "consumed"],
		["unknown lifecycle", "repairing", undefined],
	] as const)(
		"keeps %s recovery state and releases a stale erasure claim",
		async (_, lifecycle, payload_state) => {
			const database_path = await make_database_path();
			const runtime = make_runtime(database_path);
			const thread_id = `thread_pending_${lifecycle}_${payload_state ?? "missing"}`;
			const message_id = `operation_${thread_id}`;

			try {
				const result = await runtime.runPromise(
					Effect.gen(function* () {
						const erasure = yield* ThreadErasure;

						yield* SeedThread(thread_id);
						yield* SeedOperation({ lifecycle, message_id, thread_id });
						if (payload_state !== undefined)
							yield* SeedPayload(thread_id, message_id, payload_state);

						const expired = yield* erasure.CleanupExpired(cutoff, deleted_at);
						const before_resume = yield* ReadThreadState(thread_id);
						yield* SeedClaim(thread_id);
						const resumed = yield* erasure.ResumeClaimed(deleted_at);

						return {
							before_resume,
							expired,
							resumed,
							state: yield* ReadThreadState(thread_id),
						};
					}),
				);

				expect(result.expired).toEqual([]);
				expect(result.before_resume.claims).toEqual([]);
				expect(result.resumed).toEqual([]);
				expect(result.state.claims).toEqual([]);
				expect(result.state.threads).toHaveLength(1);
				expect(result.state.operations).toHaveLength(1);
				expect(result.state.payloads).toHaveLength(payload_state === undefined ? 0 : 1);
				expect(result.state.tombstones).toEqual([]);
				expect(result.state.events).toEqual([]);
			} finally {
				await runtime.dispose();
			}
		},
	);

	it.each([
		["claimed without a payload", "claimed", undefined, "replace"],
		["claimed review without a payload", "claimed", undefined, "review"],
		["committed consumed payload", "committed", "consumed", "replace"],
		["rejected consumed payload", "rejected", "consumed", "replace"],
	] as const)("erases %s", async (_, lifecycle, payload_state, action) => {
		const database_path = await make_database_path();
		const runtime = make_runtime(database_path);
		const thread_id = `thread_settled_${lifecycle}_${action}`;
		const message_id = `operation_${thread_id}`;

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const erasure = yield* ThreadErasure;

					yield* SeedThread(thread_id);
					yield* SeedOperation({ action, lifecycle, message_id, thread_id });
					if (payload_state !== undefined)
						yield* SeedPayload(thread_id, message_id, payload_state);

					const erased = yield* erasure.CleanupExpired(cutoff, deleted_at);

					return { erased, state: yield* ReadThreadState(thread_id) };
				}),
			);

			expect(result.erased).toEqual([thread_id]);
			expect(result.state.claims).toEqual([]);
			expect(result.state.threads).toEqual([]);
			expect(result.state.operations).toEqual([]);
			expect(result.state.payloads).toEqual([]);
			expect(result.state.tombstones).toEqual([{ deleted_at, thread_id }]);
			expect(result.state.events.map((event) => event.event_type)).toEqual(["thread.erased"]);
		} finally {
			await runtime.dispose();
		}
	});

	it("erases after the final payload consume settles a committed operation", async () => {
		const database_path = await make_database_path();
		const runtime = make_runtime(database_path);
		const thread_id = "thread_consumed_after_commit";
		const message_id = "operation_consumed_after_commit";
		const input = stage_input(thread_id, message_id);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const erasure = yield* ThreadErasure;
					const payloads = yield* WorkspaceMutationPayloadStore;

					yield* SeedThread(thread_id);
					yield* SeedOperation({ lifecycle: "claimed", message_id, thread_id });
					yield* payloads.Stage(input);

					const pending = yield* erasure.CleanupExpired(cutoff, deleted_at);
					yield* database.client
						.update(WorkspaceChangeOperations)
						.set({ lifecycle: "committed", updated_at: deleted_at });
					yield* payloads.Consume(consume_input(input));
					const erased = yield* erasure.CleanupExpired(cutoff, deleted_at);

					return { erased, pending, state: yield* ReadThreadState(thread_id) };
				}),
			);

			expect(result.pending).toEqual([]);
			expect(result.erased).toEqual([thread_id]);
			expect(result.state.events.map((event) => event.event_type)).toEqual(["thread.erased"]);
		} finally {
			await runtime.dispose();
		}
	});

	it("retries ClaimExpired after an overlapping payload Stage commits", async () => {
		const database_path = await make_database_path();
		const stage_hold = await make_transaction_probe(1, "after");
		const erasure_retry = await make_transaction_probe(2, "before");
		const erasure_runtime = make_erasure_runtime(database_path, {
			probe: erasure_retry,
		});
		const payload_runtime = make_payload_runtime(database_path, stage_hold);
		const thread_id = "thread_stage_snapshot_race";
		const message_id = "operation_stage_snapshot_race";
		const input = stage_input(thread_id, message_id);

		try {
			await erasure_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					yield* database.client.run("PRAGMA busy_timeout = 0");
					yield* SeedThread(thread_id);
					yield* SeedOperation({ lifecycle: "claimed", message_id, thread_id });
				}),
			);
			await payload_runtime.runPromise(
				Effect.service(Database).pipe(
					Effect.flatMap((database) => database.client.run("PRAGMA busy_timeout = 0")),
				),
			);

			let stage_settled = false;
			const stage = payload_runtime
				.runPromise(
					Effect.service(WorkspaceMutationPayloadStore).pipe(
						Effect.flatMap((payloads) => payloads.Stage(input)),
					),
				)
				.then(
					(value) => ({ status: "success" as const, value }),
					(error) => ({ error, status: "failure" as const }),
				)
				.finally(() => {
					stage_settled = true;
				});

			await wait_for_transaction(stage_hold);

			let cleanup_settled = false;
			const cleanup = erasure_runtime
				.runPromise(
					Effect.service(ThreadErasure).pipe(
						Effect.flatMap((erasure) => erasure.CleanupExpired(cutoff, deleted_at)),
					),
				)
				.then(
					(value) => ({ status: "success" as const, value }),
					(error) => ({ error, status: "failure" as const }),
				)
				.finally(() => {
					cleanup_settled = true;
				});

			await wait_for_transaction(erasure_retry);

			expect(stage_settled).toBe(false);
			expect(cleanup_settled).toBe(false);
			await continue_transaction(stage_hold);
			expect(await within_timeout(stage)).toEqual({
				status: "success",
				value: { status: "staged" },
			});
			await continue_transaction(erasure_retry);
			expect(await within_timeout(cleanup)).toEqual({ status: "success", value: [] });
			const state = await erasure_runtime.runPromise(ReadThreadState(thread_id));

			expect(state.claims).toEqual([]);
			expect(state.threads).toHaveLength(1);
			expect(state.operations).toMatchObject([{ lifecycle: "claimed", message_id }]);
			expect(state.payloads).toMatchObject([{ message_id, state: "available", thread_id }]);
			expect(state.events).toEqual([]);
		} finally {
			await continue_transaction(stage_hold);
			await continue_transaction(erasure_retry);
			await Promise.all([erasure_runtime.dispose(), payload_runtime.dispose()]);
		}
	}, 10_000);

	it("retries MarkApplied after an overlapping ClaimExpired commit", async () => {
		const database_path = await make_database_path();
		const claim_hold = await make_transaction_probe(1, "after");
		const apply_retry = await make_transaction_probe(2, "before");
		const quiesce_started = await Effect.runPromise(Deferred.make<void>());
		const quiesce_release = await Effect.runPromise(Deferred.make<void>());
		const quiescer = Layer.succeed(ThreadResourceQuiescer, {
			Quiesce: () =>
				Deferred.succeed(quiesce_started, undefined).pipe(
					Effect.andThen(Deferred.await(quiesce_release)),
				),
		});
		const erasure_runtime = make_erasure_runtime(database_path, {
			probe: claim_hold,
			quiescer,
		});
		const repository_runtime = make_workspace_change_runtime(database_path, apply_retry);
		const thread_id = "thread_apply_snapshot_race";
		const message_id = "operation_apply_snapshot_race";

		try {
			await erasure_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					yield* database.client.run("PRAGMA busy_timeout = 0");
					yield* SeedThread(thread_id);
					yield* SeedOperation({ lifecycle: "claimed", message_id, thread_id });
				}),
			);
			await repository_runtime.runPromise(
				Effect.service(Database).pipe(
					Effect.flatMap((database) => database.client.run("PRAGMA busy_timeout = 0")),
				),
			);

			const cleanup = erasure_runtime.runPromise(
				Effect.service(ThreadErasure).pipe(
					Effect.flatMap((erasure) => erasure.CleanupExpired(cutoff, deleted_at)),
				),
			);

			await within_timeout(
				Effect.runPromise(Deferred.await(claim_hold.transaction_reached)),
				"ClaimExpired first transaction",
			);

			let apply_settled = false;
			const apply = repository_runtime
				.runPromise(
					Effect.service(WorkspaceChangeRepository).pipe(
						Effect.flatMap((repository) =>
							repository.MarkApplied({
								_tag: "replace",
								message_id,
								result_identity: identity(bytes(`after:${message_id}`)),
							}),
						),
						Effect.exit,
					),
				)
				.finally(() => {
					apply_settled = true;
				});

			const apply_progress = await within_timeout(
				Promise.race([
					Effect.runPromise(Deferred.await(apply_retry.transaction_reached)).then(() => ({
						_tag: "retry" as const,
					})),
					apply.then((result) => ({ _tag: "settled" as const, result })),
				]),
				"MarkApplied second transaction",
			);

			expect(apply_progress).toEqual({ _tag: "retry" });
			expect(apply_settled).toBe(false);
			await continue_transaction(claim_hold);
			await within_timeout(
				Effect.runPromise(Deferred.await(quiesce_started)),
				"Thread erasure quiescence",
			);
			await continue_transaction(apply_retry);
			const apply_result = await within_timeout(apply);
			const state_before_erasure = await erasure_runtime.runPromise(
				ReadThreadState(thread_id),
			);

			expect(JSON.stringify(apply_result)).toContain("WorkspaceChangeTransitionError");
			expect(JSON.stringify(apply_result)).not.toContain("SQLITE_BUSY");
			expect(state_before_erasure.claims).toHaveLength(1);
			expect(state_before_erasure.operations).toMatchObject([
				{ lifecycle: "claimed", message_id },
			]);
			await Effect.runPromise(Deferred.succeed(quiesce_release, undefined));
			expect(await within_timeout(cleanup)).toEqual([thread_id]);
			const state = await erasure_runtime.runPromise(ReadThreadState(thread_id));

			expect(state.threads).toEqual([]);
			expect(state.operations).toEqual([]);
			expect(state.events.map((event) => event.event_type)).toEqual(["thread.erased"]);
		} finally {
			await continue_transaction(claim_hold);
			await continue_transaction(apply_retry);
			await Effect.runPromise(Deferred.succeed(quiesce_release, undefined));
			await Promise.all([erasure_runtime.dispose(), repository_runtime.dispose()]);
		}
	}, 10_000);

	it("serializes a claimed erasure before payload staging without deleting its operation first", async () => {
		const database_path = await make_database_path();
		const quiesce_started = await Effect.runPromise(Deferred.make<void>());
		const quiesce_release = await Effect.runPromise(Deferred.make<void>());
		const quiescer = Layer.succeed(ThreadResourceQuiescer, {
			Quiesce: () =>
				Deferred.succeed(quiesce_started, undefined).pipe(
					Effect.andThen(Deferred.await(quiesce_release)),
				),
		});
		const erasure_runtime = make_runtime(database_path, quiescer);
		const payload_runtime = make_payload_runtime(database_path);
		const thread_id = "thread_stage_race";
		const message_id = "operation_stage_race";
		const input = stage_input(thread_id, message_id);

		try {
			await erasure_runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedThread(thread_id);
					yield* SeedOperation({ lifecycle: "claimed", message_id, thread_id });
				}),
			);

			const cleanup = erasure_runtime.runPromise(
				Effect.gen(function* () {
					return yield* (yield* ThreadErasure).CleanupExpired(cutoff, deleted_at);
				}),
			);
			await Effect.runPromise(Deferred.await(quiesce_started));
			const stage = await payload_runtime.runPromise(
				Effect.gen(function* () {
					return yield* (yield* WorkspaceMutationPayloadStore)
						.Stage(input)
						.pipe(Effect.exit);
				}),
			);
			await Effect.runPromise(Deferred.succeed(quiesce_release, undefined));
			const erased = await cleanup;
			const state = await erasure_runtime.runPromise(ReadThreadState(thread_id));

			expect(JSON.stringify(stage)).toContain("WorkspaceMutationPayloadStoreUnavailable");
			expect(erased).toEqual([thread_id]);
			expect(state.operations).toEqual([]);
			expect(state.payloads).toEqual([]);
			expect(state.events.map((event) => event.event_type)).toEqual(["thread.erased"]);
		} finally {
			await Promise.all([erasure_runtime.dispose(), payload_runtime.dispose()]);
		}
	});

	it("releases a stale claim before final payload consumption and a later cleanup erases", async () => {
		const database_path = await make_database_path();
		const quiesce_started = await Effect.runPromise(Deferred.make<void>());
		const quiesce_release = await Effect.runPromise(Deferred.make<void>());
		const quiescer = Layer.succeed(ThreadResourceQuiescer, {
			Quiesce: () =>
				Deferred.succeed(quiesce_started, undefined).pipe(
					Effect.andThen(Deferred.await(quiesce_release)),
				),
		});
		const erasure_runtime = make_runtime(database_path, quiescer);
		const payload_runtime = make_payload_runtime(database_path);
		const thread_id = "thread_consume_race";
		const message_id = "operation_consume_race";
		const input = stage_input(thread_id, message_id);

		try {
			await erasure_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					yield* SeedThread(thread_id);
					yield* SeedOperation({ lifecycle: "claimed", message_id, thread_id });
					yield* (yield* WorkspaceMutationPayloadStore).Stage(input);
					yield* database.client
						.update(WorkspaceChangeOperations)
						.set({ lifecycle: "committed", updated_at: deleted_at });
					yield* SeedClaim(thread_id);
				}),
			);

			const resume = erasure_runtime.runPromise(
				Effect.gen(function* () {
					return yield* (yield* ThreadErasure).ResumeClaimed(deleted_at);
				}),
			);
			await Effect.runPromise(Deferred.await(quiesce_started));
			const blocked_consume = await payload_runtime.runPromise(
				Effect.gen(function* () {
					return yield* (yield* WorkspaceMutationPayloadStore)
						.Consume(consume_input(input))
						.pipe(Effect.exit);
				}),
			);
			await Effect.runPromise(Deferred.succeed(quiesce_release, undefined));
			const resumed = await resume;
			const state_after_release = await erasure_runtime.runPromise(
				ReadThreadState(thread_id),
			);
			const consumed = await payload_runtime.runPromise(
				Effect.gen(function* () {
					yield* (yield* WorkspaceMutationPayloadStore).Consume(consume_input(input));
				}),
			);
			const erased = await erasure_runtime.runPromise(
				Effect.gen(function* () {
					return yield* (yield* ThreadErasure).CleanupExpired(cutoff, deleted_at);
				}),
			);
			const state = await erasure_runtime.runPromise(ReadThreadState(thread_id));

			expect(JSON.stringify(blocked_consume)).toContain(
				"WorkspaceMutationPayloadStoreUnavailable",
			);
			expect(resumed).toEqual([]);
			expect(state_after_release.claims).toEqual([]);
			expect(state_after_release.operations).toHaveLength(1);
			expect(state_after_release.payloads).toHaveLength(1);
			expect(consumed).toBeUndefined();
			expect(erased).toEqual([thread_id]);
			expect(state.events.map((event) => event.event_type)).toEqual(["thread.erased"]);
		} finally {
			await Promise.all([erasure_runtime.dispose(), payload_runtime.dispose()]);
		}
	});
});
