import { createHash } from "node:crypto";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { NodeCrypto } from "@effect/platform-node-shared";
import { Deferred, Effect, Exit, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import {
	ThreadErasureClaims,
	Threads,
	ThreadTombstones,
	WorkspaceChangeOperations,
	WorkspaceChangeSnapshots,
	WorkspaceChanges,
} from "../../modules/backend/src/persistence/tables";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/metadata";
import {
	WorkspaceSnapshotStore,
	WorkspaceSnapshotStoreLive,
} from "../../modules/backend/src/workspace/snapshot-store";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];

type RetryProbe = {
	readonly continue_second_attempt: Deferred.Deferred<void>;
	readonly second_attempt_started: Deferred.Deferred<void>;
};

function identity(content: Uint8Array) {
	return {
		algorithm: "sha256" as const,
		byte_count: content.byteLength,
		content_hash: createHash("sha256").update(content).digest("hex"),
	};
}

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-workspace-snapshot-store-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

function make_metadata_layer() {
	return Layer.succeed(RuntimeMetadata, {
		instance_id: "workspace_snapshot_store_test",
		MakeId: (prefix) => Effect.succeed(`${prefix}_1`),
		Now: Effect.succeed("2026-07-11T20:00:00.000Z"),
	});
}

function make_test_database_layer(database_path: string, retry_probe?: RetryProbe) {
	const database_layer = make_database_layer({ database_path, migrations_path });

	if (!retry_probe) {
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

							if (callback_count !== 2) {
								return operation(transaction);
							}

							return Deferred.succeed(
								retry_probe.second_attempt_started,
								undefined,
							).pipe(
								Effect.andThen(Deferred.await(retry_probe.continue_second_attempt)),
								Effect.andThen(operation(transaction)),
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

function make_runtime(database_path: string, retry_probe?: RetryProbe) {
	const infrastructure = Layer.mergeAll(
		make_test_database_layer(database_path, retry_probe),
		make_metadata_layer(),
		NodeCrypto.layer,
	);

	return ManagedRuntime.make(WorkspaceSnapshotStoreLive.pipe(Layer.provideMerge(infrastructure)));
}

async function make_retry_probe(): Promise<RetryProbe> {
	return {
		continue_second_attempt: await Effect.runPromise(Deferred.make<void>()),
		second_attempt_started: await Effect.runPromise(Deferred.make<void>()),
	};
}

async function hold_sqlite_write_lock(database_path: string) {
	const acquired = await Effect.runPromise(Deferred.make<void>());
	const released = await Effect.runPromise(
		Deferred.make<
			| {
					claimed_at: string;
					thread_id: string;
			  }
			| undefined
		>(),
	);
	const runtime = ManagedRuntime.make(make_database_layer({ database_path, migrations_path }));
	let lock_released = false;
	const held = runtime.runPromise(
		Effect.scoped(
			Effect.gen(function* () {
				const database = yield* Database;
				let committed = false;

				yield* Effect.addFinalizer(() =>
					committed ? Effect.void : database.client.run("ROLLBACK").pipe(Effect.ignore),
				);
				yield* database.client.run("PRAGMA busy_timeout = 0");
				yield* database.client.run("BEGIN IMMEDIATE");
				yield* Deferred.succeed(acquired, undefined);
				const erasure_claim = yield* Deferred.await(released);

				if (erasure_claim) {
					yield* database.client.insert(ThreadErasureClaims).values(erasure_claim);
				}

				yield* database.client.run("COMMIT");

				committed = true;
			}),
		),
	);

	await Effect.runPromise(Deferred.await(acquired));

	return {
		Release: async (erasure_claim?: { claimed_at: string; thread_id: string }) => {
			if (!lock_released) {
				lock_released = true;
				await Effect.runPromise(Deferred.succeed(released, erasure_claim));
			}

			await held;
			await runtime.dispose();
		},
	};
}

function SeedThread(thread_id: string) {
	return Effect.gen(function* () {
		const database = yield* Database;

		yield* database.client.insert(Threads).values({
			created_at: "2026-07-11T20:00:00.000Z",
			thread_id,
			title: thread_id,
			title_source: "initial",
			updated_at: "2026-07-11T20:00:00.000Z",
		});
	});
}

function replace_message_id(change_id: string) {
	return `message_snapshot_${change_id}`;
}

function rollback_message_id(change_id: string, variant = "canonical") {
	return `message_rollback_${variant}_${change_id}`;
}

function after_identity(change_id: string) {
	return identity(new TextEncoder().encode(`after:${change_id}`));
}

function SeedReplaceClaim(
	change_id: string,
	thread_id: string,
	content: Uint8Array,
	lifecycle: "applied" | "claimed" = "claimed",
) {
	return Effect.gen(function* () {
		const database = yield* Database;
		const message_id = replace_message_id(change_id);
		const now = "2026-07-11T20:00:00.000Z";

		yield* database.client.insert(WorkspaceChangeOperations).values({
			action: "replace",
			agent_id: `agent_${change_id}`,
			change_id,
			created_at: now,
			expected_identity_json: JSON.stringify(identity(content)),
			lifecycle,
			message_id,
			path: `src/${change_id}.ts`,
			raw_origin_json: JSON.stringify({
				provider: "codex",
				reference: `origin_${change_id}`,
			}),
			request_fingerprint: createHash("sha256").update(message_id).digest("hex"),
			result_identity_json: JSON.stringify(after_identity(change_id)),
			run_id: `run_${change_id}`,
			sent_at: now,
			thread_id,
			updated_at: now,
			workspace_id: `workspace_${thread_id}`,
		});
	});
}

function CommitReplace(
	change_id: string,
	thread_id: string,
	content: Uint8Array,
	rollback_state: "available" | "consumed" = "available",
) {
	return Effect.gen(function* () {
		const database = yield* Database;
		const message_id = replace_message_id(change_id);
		const now = "2026-07-11T20:00:01.000Z";

		yield* database.client.run(
			`UPDATE workspace_change_operations SET lifecycle = 'committed', updated_at = '${now}' WHERE message_id = '${message_id}'`,
		);
		yield* database.client.insert(WorkspaceChanges).values({
			after_identity_json: JSON.stringify(after_identity(change_id)),
			agent_id: `agent_${change_id}`,
			before_identity_json: JSON.stringify(identity(content)),
			change_id,
			created_at: now,
			path: `src/${change_id}.ts`,
			review_state: "needs_review",
			rollback_state,
			run_id: `run_${change_id}`,
			source_command_id: message_id,
			thread_id,
			updated_at: now,
			version: 1,
			workspace_id: `workspace_${thread_id}`,
		});
	});
}

function RejectReplace(change_id: string) {
	return Effect.gen(function* () {
		const database = yield* Database;

		yield* database.client.run(
			`UPDATE workspace_change_operations SET lifecycle = 'rejected' WHERE message_id = '${replace_message_id(change_id)}'`,
		);
	});
}

function SeedChangeProjection(options: {
	readonly before: Uint8Array;
	readonly change_id: string;
	readonly source_command_id: string;
	readonly thread_id: string;
}) {
	return Effect.gen(function* () {
		const database = yield* Database;
		const now = "2026-07-11T20:00:01.000Z";

		yield* database.client.insert(WorkspaceChanges).values({
			after_identity_json: JSON.stringify(after_identity(options.change_id)),
			agent_id: `agent_${options.change_id}`,
			before_identity_json: JSON.stringify(identity(options.before)),
			change_id: options.change_id,
			created_at: now,
			path: `src/${options.change_id}.ts`,
			review_state: "needs_review",
			rollback_state: "available",
			run_id: `run_${options.change_id}`,
			source_command_id: options.source_command_id,
			thread_id: options.thread_id,
			updated_at: now,
			version: 1,
			workspace_id: `workspace_${options.thread_id}`,
		});
	});
}

function SeedRollbackOperation(
	change_id: string,
	thread_id: string,
	lifecycle: "applied" | "claimed" | "committed",
	options: {
		readonly expected_identity?: ReturnType<typeof identity>;
		readonly variant?: string;
	} = {},
) {
	return Effect.gen(function* () {
		const database = yield* Database;
		const message_id = rollback_message_id(change_id, options.variant);
		const now = "2026-07-11T20:00:02.000Z";

		yield* database.client.insert(WorkspaceChangeOperations).values({
			action: "rollback",
			change_id,
			created_at: now,
			expected_identity_json: JSON.stringify(
				options.expected_identity ?? after_identity(change_id),
			),
			lifecycle,
			message_id,
			request_fingerprint: createHash("sha256").update(message_id).digest("hex"),
			sent_at: now,
			thread_id,
			updated_at: now,
		});
	});
}

function CommitRollback(change_id: string, variant = "canonical") {
	return Effect.gen(function* () {
		const database = yield* Database;
		const message_id = rollback_message_id(change_id, variant);
		const now = "2026-07-11T20:00:03.000Z";

		yield* database.client.run(
			`UPDATE workspace_change_operations SET lifecycle = 'committed', updated_at = '${now}' WHERE message_id = '${message_id}'`,
		);
		yield* database.client.run(
			`UPDATE workspace_changes SET rollback_state = 'consumed', rolled_back_at = '${now}', updated_at = '${now}', version = version + 1 WHERE change_id = '${change_id}'`,
		);
	});
}

function stage_input(change_id: string, thread_id: string, content: Uint8Array) {
	return { change_id, content, expected_identity: identity(content), thread_id };
}

function read_input(change_id: string, thread_id: string, content: Uint8Array) {
	return { change_id, expected_identity: identity(content), thread_id };
}

function consume_input(change_id: string, thread_id: string, variant = "canonical") {
	return {
		change_id,
		rollback_message_id: rollback_message_id(change_id, variant),
		thread_id,
	};
}

function discard_rejected_replace_input(change_id: string, thread_id: string, content: Uint8Array) {
	return {
		change_id,
		expected_identity: identity(content),
		replace_message_id: replace_message_id(change_id),
		thread_id,
	};
}

function expect_snapshot_unavailable_without_bytes(exit: unknown, content: Uint8Array) {
	const serialized = JSON.stringify(exit);
	const source = new TextDecoder().decode(content);

	expect(serialized).toContain("WorkspaceSnapshotStoreUnavailable");
	expect(serialized).not.toContain(source);
}

function within_timeout<A>(promise: Promise<A>) {
	return Promise.race([
		promise,
		new Promise<never>((_resolve, reject) =>
			setTimeout(() => reject(new Error("Snapshot operation timed out")), 5_000),
		),
	]);
}

async function wait_for_second_attempt(retry_probe: RetryProbe) {
	await within_timeout(Effect.runPromise(Deferred.await(retry_probe.second_attempt_started)));
}

async function continue_second_attempt(retry_probe: RetryProbe) {
	await Effect.runPromise(Deferred.succeed(retry_probe.continue_second_attempt, undefined));
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("WorkspaceSnapshotStore", () => {
	it("accepts repeated rejected cleanup when Changed happened before snapshot staging", async () => {
		const runtime = make_runtime(await make_database_path());
		const change_id = "change_rejected_snapshot_absent";
		const thread_id = "thread_rejected_snapshot_absent";
		const content = new TextEncoder().encode("absent rejected snapshot before");

		try {
			const rows = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const snapshots = yield* WorkspaceSnapshotStore;
					const input = discard_rejected_replace_input(change_id, thread_id, content);

					yield* SeedThread(thread_id);
					yield* SeedReplaceClaim(change_id, thread_id, content);
					yield* database.client.run(
						`UPDATE workspace_change_operations SET lifecycle = 'rejected' WHERE message_id = '${replace_message_id(change_id)}'`,
					);
					yield* snapshots.DiscardRejectedReplace(input);
					yield* snapshots.DiscardRejectedReplace(input);

					return yield* database.client.select().from(WorkspaceChangeSnapshots);
				}),
			);

			expect(rows).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});

	it("discards a rejected replace snapshot across restart and never resumes it", async () => {
		const database_path = await make_database_path();
		const change_id = "change_rejected_snapshot_restart";
		const thread_id = "thread_rejected_snapshot_restart";
		const content = new TextEncoder().encode("rejected snapshot before");
		const first_runtime = make_runtime(database_path);

		try {
			await first_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const snapshots = yield* WorkspaceSnapshotStore;

					yield* SeedThread(thread_id);
					yield* SeedReplaceClaim(change_id, thread_id, content);
					yield* snapshots.Stage(stage_input(change_id, thread_id, content));
					yield* database.client.run(
						`UPDATE workspace_change_operations SET lifecycle = 'rejected' WHERE message_id = '${replace_message_id(change_id)}'`,
					);
					yield* snapshots.DiscardRejectedReplace(
						discard_rejected_replace_input(change_id, thread_id, content),
					);
				}),
			);
		} finally {
			await first_runtime.dispose();
		}

		const second_runtime = make_runtime(database_path);

		try {
			const result = await second_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const snapshots = yield* WorkspaceSnapshotStore;
					const input = discard_rejected_replace_input(change_id, thread_id, content);

					yield* snapshots.DiscardRejectedReplace(input);

					return {
						resume: yield* snapshots
							.Resume(read_input(change_id, thread_id, content))
							.pipe(Effect.exit),
						row: (yield* database.client.select().from(WorkspaceChangeSnapshots))[0],
						stage: yield* snapshots
							.Stage(stage_input(change_id, thread_id, content))
							.pipe(Effect.exit),
					};
				}),
			);

			expect(result.row).toMatchObject({
				state: "consumed",
				byte_count: null,
				content: null,
				content_hash: null,
			});
			expect(JSON.stringify(result.resume)).toContain("WorkspaceSnapshotStoreUnavailable");
			expect(JSON.stringify(result.stage)).toContain("WorkspaceSnapshotStoreUnavailable");
		} finally {
			await second_runtime.dispose();
		}
	});

	it("converges two runtimes that discard the same rejected replace snapshot", async () => {
		const database_path = await make_database_path();
		const change_id = "change_rejected_snapshot_concurrent";
		const thread_id = "thread_rejected_snapshot_concurrent";
		const content = new TextEncoder().encode("concurrent rejected snapshot before");
		const first_runtime = make_runtime(database_path);
		const second_runtime = make_runtime(database_path);

		try {
			await first_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const snapshots = yield* WorkspaceSnapshotStore;

					yield* SeedThread(thread_id);
					yield* SeedReplaceClaim(change_id, thread_id, content);
					yield* snapshots.Stage(stage_input(change_id, thread_id, content));
					yield* database.client.run(
						`UPDATE workspace_change_operations SET lifecycle = 'rejected' WHERE message_id = '${replace_message_id(change_id)}'`,
					);
				}),
			);
			const input = discard_rejected_replace_input(change_id, thread_id, content);

			await Promise.all(
				[first_runtime, second_runtime].map((runtime) =>
					runtime.runPromise(
						Effect.service(WorkspaceSnapshotStore).pipe(
							Effect.flatMap((snapshots) => snapshots.DiscardRejectedReplace(input)),
						),
					),
				),
			);
			const [row] = await first_runtime.runPromise(
				Effect.service(Database).pipe(
					Effect.flatMap((database) =>
						database.client.select().from(WorkspaceChangeSnapshots),
					),
				),
			);

			expect(row).toMatchObject({ state: "consumed", content: null });
		} finally {
			await Promise.all([first_runtime.dispose(), second_runtime.dispose()]);
		}
	});

	it.each(["message", "thread", "expected_identity"] as const)(
		"rejects an absent snapshot with the wrong %s without exposing bytes",
		async (variant) => {
			const runtime = make_runtime(await make_database_path());
			const change_id = `change_rejected_snapshot_wrong_${variant}`;
			const thread_id = `thread_rejected_snapshot_wrong_${variant}`;
			const content = new TextEncoder().encode(`private rejected snapshot wrong ${variant}`);

			try {
				const result = await runtime.runPromise(
					Effect.gen(function* () {
						const database = yield* Database;
						const snapshots = yield* WorkspaceSnapshotStore;
						const canonical = discard_rejected_replace_input(
							change_id,
							thread_id,
							content,
						);
						const input =
							variant === "message"
								? {
										...canonical,
										replace_message_id: `wrong_${canonical.replace_message_id}`,
									}
								: variant === "thread"
									? { ...canonical, thread_id: `wrong_${thread_id}` }
									: {
											...canonical,
											expected_identity: identity(
												new TextEncoder().encode("different identity"),
											),
										};

						yield* SeedThread(thread_id);
						yield* SeedReplaceClaim(change_id, thread_id, content);
						yield* RejectReplace(change_id);

						return {
							exit: yield* snapshots.DiscardRejectedReplace(input).pipe(Effect.exit),
							rows: yield* database.client.select().from(WorkspaceChangeSnapshots),
						};
					}),
				);

				expect_snapshot_unavailable_without_bytes(result.exit, content);
				expect(result.rows).toEqual([]);
			} finally {
				await runtime.dispose();
			}
		},
	);

	it.each(["claimed", "applied", "committed"] as const)(
		"rejects an absent snapshot while the replace lifecycle is %s",
		async (lifecycle) => {
			const runtime = make_runtime(await make_database_path());
			const change_id = `change_rejected_snapshot_lifecycle_${lifecycle}`;
			const thread_id = `thread_rejected_snapshot_lifecycle_${lifecycle}`;
			const content = new TextEncoder().encode(`private lifecycle snapshot ${lifecycle}`);

			try {
				const result = await runtime.runPromise(
					Effect.gen(function* () {
						const database = yield* Database;
						const snapshots = yield* WorkspaceSnapshotStore;

						yield* SeedThread(thread_id);
						yield* SeedReplaceClaim(change_id, thread_id, content);
						yield* database.client.run(
							`UPDATE workspace_change_operations SET lifecycle = '${lifecycle}' WHERE message_id = '${replace_message_id(change_id)}'`,
						);

						return {
							exit: yield* snapshots
								.DiscardRejectedReplace(
									discard_rejected_replace_input(change_id, thread_id, content),
								)
								.pipe(Effect.exit),
							rows: yield* database.client.select().from(WorkspaceChangeSnapshots),
						};
					}),
				);

				expect_snapshot_unavailable_without_bytes(result.exit, content);
				expect(result.rows).toEqual([]);
			} finally {
				await runtime.dispose();
			}
		},
	);

	it.each(["review", "rollback"] as const)(
		"rejects an absent snapshot for a rejected %s operation",
		async (action) => {
			const runtime = make_runtime(await make_database_path());
			const change_id = `change_rejected_snapshot_action_${action}`;
			const thread_id = `thread_rejected_snapshot_action_${action}`;
			const content = new TextEncoder().encode(`private action snapshot ${action}`);

			try {
				const result = await runtime.runPromise(
					Effect.gen(function* () {
						const database = yield* Database;
						const snapshots = yield* WorkspaceSnapshotStore;

						yield* SeedThread(thread_id);
						yield* SeedReplaceClaim(change_id, thread_id, content);
						yield* database.client.run(
							`UPDATE workspace_change_operations SET action = '${action}', lifecycle = 'rejected' WHERE message_id = '${replace_message_id(change_id)}'`,
						);

						return yield* snapshots
							.DiscardRejectedReplace(
								discard_rejected_replace_input(change_id, thread_id, content),
							)
							.pipe(Effect.exit);
					}),
				);

				expect_snapshot_unavailable_without_bytes(result, content);
			} finally {
				await runtime.dispose();
			}
		},
	);

	it.each(["change_id", "source_command_id"] as const)(
		"rejects an absent snapshot with a forged projection matched by %s",
		async (alias) => {
			const runtime = make_runtime(await make_database_path());
			const change_id = `change_rejected_snapshot_projection_${alias}`;
			const thread_id = `thread_rejected_snapshot_projection_${alias}`;
			const content = new TextEncoder().encode(`private projection snapshot ${alias}`);

			try {
				const result = await runtime.runPromise(
					Effect.gen(function* () {
						const snapshots = yield* WorkspaceSnapshotStore;

						yield* SeedThread(thread_id);
						yield* SeedReplaceClaim(change_id, thread_id, content);
						yield* RejectReplace(change_id);
						yield* SeedChangeProjection({
							before: content,
							change_id: alias === "change_id" ? change_id : `alias_${change_id}`,
							source_command_id:
								alias === "source_command_id"
									? replace_message_id(change_id)
									: `forged_source_${change_id}`,
							thread_id,
						});

						return yield* snapshots
							.DiscardRejectedReplace(
								discard_rejected_replace_input(change_id, thread_id, content),
							)
							.pipe(Effect.exit);
					}),
				);

				expect_snapshot_unavailable_without_bytes(result, content);
			} finally {
				await runtime.dispose();
			}
		},
	);

	it.each(["content", "hash", "count"] as const)(
		"rejects rejected cleanup when available snapshot %s is corrupt",
		async (corruption) => {
			const runtime = make_runtime(await make_database_path());
			const change_id = `change_rejected_snapshot_corrupt_${corruption}`;
			const thread_id = `thread_rejected_snapshot_corrupt_${corruption}`;
			const content = new TextEncoder().encode(`private corrupt snapshot ${corruption}`);

			try {
				const result = await runtime.runPromise(
					Effect.gen(function* () {
						const database = yield* Database;
						const snapshots = yield* WorkspaceSnapshotStore;

						yield* SeedThread(thread_id);
						yield* SeedReplaceClaim(change_id, thread_id, content);
						yield* snapshots.Stage(stage_input(change_id, thread_id, content));
						yield* RejectReplace(change_id);
						yield* database.client.run("PRAGMA ignore_check_constraints = ON");

						if (corruption === "content") {
							yield* database.client
								.update(WorkspaceChangeSnapshots)
								.set({ content: Buffer.from("corrupt snapshot") });
						} else if (corruption === "hash") {
							yield* database.client
								.update(WorkspaceChangeSnapshots)
								.set({ content_hash: "f".repeat(64) });
						} else {
							yield* database.client
								.update(WorkspaceChangeSnapshots)
								.set({ byte_count: content.byteLength + 1 });
						}

						yield* database.client.run("PRAGMA ignore_check_constraints = OFF");
						const exit = yield* snapshots
							.DiscardRejectedReplace(
								discard_rejected_replace_input(change_id, thread_id, content),
							)
							.pipe(Effect.exit);

						return {
							exit,
							row: (yield* database.client
								.select()
								.from(WorkspaceChangeSnapshots))[0],
						};
					}),
				);

				expect_snapshot_unavailable_without_bytes(result.exit, content);
				expect(result.row).toMatchObject({ state: "available" });
			} finally {
				await runtime.dispose();
			}
		},
	);

	it.each(["claim", "tombstone"] as const)(
		"rejects rejected cleanup after a thread erasure %s",
		async (erasure) => {
			const runtime = make_runtime(await make_database_path());
			const change_id = `change_rejected_snapshot_erasure_${erasure}`;
			const thread_id = `thread_rejected_snapshot_erasure_${erasure}`;
			const content = new TextEncoder().encode(`private erasure snapshot ${erasure}`);

			try {
				const result = await runtime.runPromise(
					Effect.gen(function* () {
						const database = yield* Database;
						const snapshots = yield* WorkspaceSnapshotStore;

						yield* SeedThread(thread_id);
						yield* SeedReplaceClaim(change_id, thread_id, content);
						yield* snapshots.Stage(stage_input(change_id, thread_id, content));
						yield* RejectReplace(change_id);

						if (erasure === "claim") {
							yield* database.client.insert(ThreadErasureClaims).values({
								claimed_at: "2026-07-11T20:00:01.000Z",
								thread_id,
							});
						} else {
							yield* database.client.run(
								`DELETE FROM threads WHERE thread_id = '${thread_id}'`,
							);
							yield* database.client.insert(ThreadTombstones).values({
								deleted_at: "2026-07-11T20:00:02.000Z",
								thread_id,
							});
						}

						const exit = yield* snapshots
							.DiscardRejectedReplace(
								discard_rejected_replace_input(change_id, thread_id, content),
							)
							.pipe(Effect.exit);

						return {
							exit,
							row: (yield* database.client
								.select()
								.from(WorkspaceChangeSnapshots))[0],
						};
					}),
				);

				expect_snapshot_unavailable_without_bytes(result.exit, content);
				expect(result.row).toMatchObject({ state: "available" });
			} finally {
				await runtime.dispose();
			}
		},
	);
	it("stages exact bytes, reads a defensive copy, retries, and survives restart", async () => {
		const database_path = await make_database_path();
		const thread_id = "thread_snapshot_restart";
		const change_id = "change_snapshot_restart";
		const content = new TextEncoder().encode("before restart");
		const first_runtime = make_runtime(database_path);

		try {
			await first_runtime.runPromise(SeedThread(thread_id));
			await first_runtime.runPromise(SeedReplaceClaim(change_id, thread_id, content));
			const result = await first_runtime.runPromise(
				Effect.gen(function* () {
					const snapshots = yield* WorkspaceSnapshotStore;
					const input = stage_input(change_id, thread_id, content);
					const staged = yield* snapshots.Stage(input);

					yield* CommitReplace(change_id, thread_id, content);
					const read = yield* snapshots.Read({
						change_id,
						expected_identity: input.expected_identity,
						thread_id,
					});

					read[0] = 0;

					return {
						existing: yield* snapshots.Stage(input),
						read_again: yield* snapshots.Read({
							change_id,
							expected_identity: input.expected_identity,
							thread_id,
						}),
						staged,
					};
				}),
			);

			expect(result.staged).toEqual({ status: "staged" });
			expect(result.existing).toEqual({ status: "existing" });
			expect(result.read_again).toEqual(content);
		} finally {
			await first_runtime.dispose();
		}

		const second_runtime = make_runtime(database_path);

		try {
			const read = await second_runtime.runPromise(
				Effect.service(WorkspaceSnapshotStore).pipe(
					Effect.flatMap((snapshots) =>
						snapshots.Read({
							change_id,
							expected_identity: identity(content),
							thread_id,
						}),
					),
				),
			);

			expect(read).toEqual(content);
		} finally {
			await second_runtime.dispose();
		}
	});

	it("rejects changed identities or bytes and preserves the original snapshot", async () => {
		const runtime = make_runtime(await make_database_path());
		const thread_id = "thread_snapshot_conflict";
		const change_id = "change_snapshot_conflict";
		const original = new TextEncoder().encode("before");
		const replacement = new TextEncoder().encode("after");

		try {
			await runtime.runPromise(SeedThread(thread_id));
			await runtime.runPromise(SeedReplaceClaim(change_id, thread_id, original));
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const snapshots = yield* WorkspaceSnapshotStore;

					yield* snapshots.Stage(stage_input(change_id, thread_id, original));
					const changed_bytes = yield* Effect.exit(
						snapshots.Stage(stage_input(change_id, thread_id, replacement)),
					);
					const changed_identity = yield* Effect.exit(
						snapshots.Stage({
							...stage_input(change_id, thread_id, original),
							expected_identity: {
								...identity(original),
								content_hash: "0".repeat(64),
							},
						}),
					);

					yield* CommitReplace(change_id, thread_id, original);

					return {
						changed_bytes,
						changed_identity,
						read: yield* snapshots.Read({
							change_id,
							expected_identity: identity(original),
							thread_id,
						}),
					};
				}),
			);

			expect(Exit.isFailure(result.changed_bytes)).toBe(true);
			expect(JSON.stringify(result.changed_bytes)).toContain(
				"WorkspaceSnapshotStoreUnavailable",
			);
			expect(Exit.isFailure(result.changed_identity)).toBe(true);
			expect(JSON.stringify(result.changed_identity)).toContain(
				"WorkspaceSnapshotStoreInvalid",
			);
			expect(result.read).toEqual(original);
		} finally {
			await runtime.dispose();
		}
	});

	it("resumes staged snapshots only while the canonical replace is uncommitted", async () => {
		const runtime = make_runtime(await make_database_path());
		const thread_id = "thread_snapshot_resume";
		const other_thread_id = "thread_snapshot_resume_other";
		const claimed_change_id = "change_snapshot_resume_claimed";
		const applied_change_id = "change_snapshot_resume_applied";
		const claimed_content = new TextEncoder().encode("claimed recovery bytes");
		const applied_content = new TextEncoder().encode("applied recovery bytes");
		const unrelated_identity = identity(new TextEncoder().encode("unrelated bytes"));

		try {
			await runtime.runPromise(SeedThread(thread_id));
			await runtime.runPromise(SeedThread(other_thread_id));
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const snapshots = yield* WorkspaceSnapshotStore;
					const claimed_stage = stage_input(
						claimed_change_id,
						thread_id,
						claimed_content,
					);
					const claimed_read = read_input(claimed_change_id, thread_id, claimed_content);
					const applied_stage = stage_input(
						applied_change_id,
						thread_id,
						applied_content,
					);
					const applied_read = read_input(applied_change_id, thread_id, applied_content);

					yield* SeedReplaceClaim(claimed_change_id, thread_id, claimed_content);
					const absent = yield* Effect.exit(snapshots.Resume(claimed_read));
					yield* snapshots.Stage(claimed_stage);

					const claimed = yield* snapshots.Resume(claimed_read);
					const read_before_commit = yield* Effect.exit(snapshots.Read(claimed_read));
					const wrong_identity = yield* Effect.exit(
						snapshots.Resume({
							...claimed_read,
							expected_identity: unrelated_identity,
						}),
					);
					const wrong_thread = yield* Effect.exit(
						snapshots.Resume({ ...claimed_read, thread_id: other_thread_id }),
					);

					yield* SeedReplaceClaim(
						applied_change_id,
						thread_id,
						applied_content,
						"applied",
					);
					yield* snapshots.Stage(applied_stage);
					const applied = yield* snapshots.Resume(applied_read);

					yield* CommitReplace(claimed_change_id, thread_id, claimed_content);

					return {
						absent,
						applied,
						claimed,
						committed_read: yield* snapshots.Read(claimed_read),
						committed_resume: yield* Effect.exit(snapshots.Resume(claimed_read)),
						read_before_commit,
						wrong_identity,
						wrong_thread,
					};
				}),
			);

			expect(result.claimed).toEqual(claimed_content);
			expect(result.applied).toEqual(applied_content);
			expect(result.committed_read).toEqual(claimed_content);
			for (const failure of [
				result.absent,
				result.committed_resume,
				result.read_before_commit,
				result.wrong_identity,
				result.wrong_thread,
			]) {
				expect(JSON.stringify(failure)).toContain("WorkspaceSnapshotStoreUnavailable");
			}
		} finally {
			await runtime.dispose();
		}
	});

	it("writes consumed tombstones that block all future stages", async () => {
		const runtime = make_runtime(await make_database_path());
		const thread_id = "thread_snapshot_consumed";
		const change_id = "change_snapshot_consumed";
		const content = new TextEncoder().encode("before");

		try {
			await runtime.runPromise(SeedThread(thread_id));
			await runtime.runPromise(SeedReplaceClaim(change_id, thread_id, content));
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const snapshots = yield* WorkspaceSnapshotStore;

					yield* snapshots.Stage(stage_input(change_id, thread_id, content));
					yield* CommitReplace(change_id, thread_id, content);
					yield* SeedRollbackOperation(change_id, thread_id, "applied");
					yield* snapshots.Consume(consume_input(change_id, thread_id));
					yield* CommitRollback(change_id);
					yield* snapshots.Consume(consume_input(change_id, thread_id));

					return {
						exists: yield* snapshots.Exists({ change_id, thread_id }),
						read: yield* Effect.exit(
							snapshots.Read({
								change_id,
								expected_identity: identity(content),
								thread_id,
							}),
						),
						resume: yield* Effect.exit(
							snapshots.Resume({
								change_id,
								expected_identity: identity(content),
								thread_id,
							}),
						),
						stage: yield* Effect.exit(
							snapshots.Stage(stage_input(change_id, thread_id, content)),
						),
						stored: yield* database.client.select().from(WorkspaceChangeSnapshots),
					};
				}),
			);

			expect(result.exists).toBe(false);
			expect(JSON.stringify(result.read)).toContain("WorkspaceSnapshotStoreUnavailable");
			expect(JSON.stringify(result.resume)).toContain("WorkspaceSnapshotStoreUnavailable");
			expect(JSON.stringify(result.stage)).toContain("WorkspaceSnapshotStoreUnavailable");
			expect(result.stored).toMatchObject([
				{
					byte_count: null,
					change_id,
					content: null,
					content_hash: null,
					state: "consumed",
				},
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("cannot create or recreate a snapshot after replace commit", async () => {
		const runtime = make_runtime(await make_database_path());
		const thread_id = "thread_snapshot_committed_stage";
		const missing_change_id = "change_snapshot_committed_missing";
		const consumed_change_id = "change_snapshot_committed_consumed";
		const missing_content = new TextEncoder().encode("missing committed bytes");
		const consumed_content = new TextEncoder().encode("consumed committed bytes");

		try {
			await runtime.runPromise(SeedThread(thread_id));
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const snapshots = yield* WorkspaceSnapshotStore;

					yield* SeedReplaceClaim(missing_change_id, thread_id, missing_content);
					yield* CommitReplace(missing_change_id, thread_id, missing_content);
					const missing = yield* Effect.exit(
						snapshots.Stage(stage_input(missing_change_id, thread_id, missing_content)),
					);

					yield* SeedReplaceClaim(consumed_change_id, thread_id, consumed_content);
					yield* snapshots.Stage(
						stage_input(consumed_change_id, thread_id, consumed_content),
					);
					yield* CommitReplace(consumed_change_id, thread_id, consumed_content);
					yield* SeedRollbackOperation(consumed_change_id, thread_id, "applied");
					yield* snapshots.Consume(consume_input(consumed_change_id, thread_id));
					yield* CommitRollback(consumed_change_id);
					const consumed = yield* Effect.exit(
						snapshots.Stage(
							stage_input(consumed_change_id, thread_id, consumed_content),
						),
					);

					return {
						consumed,
						missing,
						stored: yield* database.client.select().from(WorkspaceChangeSnapshots),
					};
				}),
			);

			for (const failure of [result.consumed, result.missing]) {
				expect(JSON.stringify(failure)).toContain("WorkspaceSnapshotStoreUnavailable");
			}
			expect(result.stored).toMatchObject([
				{
					byte_count: null,
					change_id: consumed_change_id,
					content: null,
					content_hash: null,
					state: "consumed",
					thread_id,
				},
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects Consume before replace and rollback lifecycles authorize it", async () => {
		const runtime = make_runtime(await make_database_path());
		const thread_id = "thread_snapshot_consume_lifecycle";
		const claimed_change_id = "change_snapshot_replace_claimed";
		const applied_change_id = "change_snapshot_replace_applied";
		const rollback_claimed_change_id = "change_snapshot_rollback_claimed";
		const content = new TextEncoder().encode("lifecycle private bytes");

		try {
			await runtime.runPromise(SeedThread(thread_id));
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const snapshots = yield* WorkspaceSnapshotStore;

					yield* SeedReplaceClaim(claimed_change_id, thread_id, content);
					yield* snapshots.Stage(stage_input(claimed_change_id, thread_id, content));
					yield* SeedRollbackOperation(claimed_change_id, thread_id, "applied");

					yield* SeedReplaceClaim(applied_change_id, thread_id, content, "applied");
					yield* snapshots.Stage(stage_input(applied_change_id, thread_id, content));
					yield* SeedRollbackOperation(applied_change_id, thread_id, "applied");

					yield* SeedReplaceClaim(rollback_claimed_change_id, thread_id, content);
					yield* snapshots.Stage(
						stage_input(rollback_claimed_change_id, thread_id, content),
					);
					yield* CommitReplace(rollback_claimed_change_id, thread_id, content);
					yield* SeedRollbackOperation(rollback_claimed_change_id, thread_id, "claimed");

					return {
						applied_replace: yield* Effect.exit(
							snapshots.Consume(consume_input(applied_change_id, thread_id)),
						),
						claimed_replace: yield* Effect.exit(
							snapshots.Consume(consume_input(claimed_change_id, thread_id)),
						),
						claimed_rollback: yield* Effect.exit(
							snapshots.Consume(consume_input(rollback_claimed_change_id, thread_id)),
						),
						stored: yield* database.client.select().from(WorkspaceChangeSnapshots),
					};
				}),
			);

			for (const failure of [
				result.applied_replace,
				result.claimed_replace,
				result.claimed_rollback,
			]) {
				expect(JSON.stringify(failure)).toContain("WorkspaceSnapshotStoreUnavailable");
			}
			expect(result.stored).toHaveLength(3);
			for (const stored of result.stored) {
				expect(stored).toMatchObject({
					byte_count: content.byteLength,
					content: Buffer.from(content),
					content_hash: identity(content).content_hash,
					state: "available",
					thread_id,
				});
			}
		} finally {
			await runtime.dispose();
		}
	});

	it("serializes authorized concurrent Consume calls across database runtimes", async () => {
		const database_path = await make_database_path();
		const first_runtime = make_runtime(database_path);
		const second_runtime = make_runtime(database_path);
		const thread_id = "thread_snapshot_consume_race";
		const change_id = "change_snapshot_consume_race";
		const content = new TextEncoder().encode("private concurrent consume bytes");

		try {
			await first_runtime.runPromise(SeedThread(thread_id));
			await first_runtime.runPromise(
				Effect.gen(function* () {
					const snapshots = yield* WorkspaceSnapshotStore;

					yield* SeedReplaceClaim(change_id, thread_id, content);
					yield* snapshots.Stage(stage_input(change_id, thread_id, content));
					yield* CommitReplace(change_id, thread_id, content);
					yield* SeedRollbackOperation(change_id, thread_id, "applied");
				}),
			);
			const consumes = await within_timeout(
				Promise.all([
					first_runtime.runPromise(
						Effect.service(WorkspaceSnapshotStore).pipe(
							Effect.flatMap((snapshots) =>
								snapshots.Consume(consume_input(change_id, thread_id)),
							),
						),
					),
					second_runtime.runPromise(
						Effect.service(WorkspaceSnapshotStore).pipe(
							Effect.flatMap((snapshots) =>
								snapshots.Consume(consume_input(change_id, thread_id)),
							),
						),
					),
				]),
			);

			expect(consumes).toEqual([undefined, undefined]);
			await first_runtime.runPromise(CommitRollback(change_id));
			await second_runtime.runPromise(
				Effect.service(WorkspaceSnapshotStore).pipe(
					Effect.flatMap((snapshots) =>
						snapshots.Consume(consume_input(change_id, thread_id)),
					),
				),
			);

			const stored = await first_runtime.runPromise(
				Effect.service(Database).pipe(
					Effect.flatMap((database) =>
						database.client.select().from(WorkspaceChangeSnapshots),
					),
				),
			);

			expect(stored).toMatchObject([
				{
					byte_count: null,
					change_id,
					content: null,
					content_hash: null,
					state: "consumed",
					thread_id,
				},
			]);
		} finally {
			await Promise.all([first_runtime.dispose(), second_runtime.dispose()]);
		}
	}, 10_000);

	it("rejects rollback authority with the wrong message or expected identity", async () => {
		const runtime = make_runtime(await make_database_path());
		const thread_id = "thread_snapshot_wrong_rollback";
		const wrong_message_change_id = "change_snapshot_wrong_rollback_message";
		const wrong_identity_change_id = "change_snapshot_wrong_rollback_identity";
		const content = new TextEncoder().encode("rollback authority private bytes");
		const wrong_identity = identity(new TextEncoder().encode("unrelated after bytes"));

		try {
			await runtime.runPromise(SeedThread(thread_id));
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const snapshots = yield* WorkspaceSnapshotStore;

					yield* SeedReplaceClaim(wrong_message_change_id, thread_id, content);
					yield* snapshots.Stage(
						stage_input(wrong_message_change_id, thread_id, content),
					);
					yield* CommitReplace(wrong_message_change_id, thread_id, content);
					yield* SeedRollbackOperation(wrong_message_change_id, thread_id, "applied");

					yield* SeedReplaceClaim(wrong_identity_change_id, thread_id, content);
					yield* snapshots.Stage(
						stage_input(wrong_identity_change_id, thread_id, content),
					);
					yield* CommitReplace(wrong_identity_change_id, thread_id, content);
					yield* SeedRollbackOperation(wrong_identity_change_id, thread_id, "applied", {
						expected_identity: wrong_identity,
					});

					return {
						stored: yield* database.client.select().from(WorkspaceChangeSnapshots),
						wrong_identity: yield* Effect.exit(
							snapshots.Consume(consume_input(wrong_identity_change_id, thread_id)),
						),
						wrong_message: yield* Effect.exit(
							snapshots.Consume({
								...consume_input(wrong_message_change_id, thread_id),
								rollback_message_id: rollback_message_id(wrong_identity_change_id),
							}),
						),
					};
				}),
			);

			for (const failure of [result.wrong_identity, result.wrong_message]) {
				expect(JSON.stringify(failure)).toContain("WorkspaceSnapshotStoreUnavailable");
			}
			expect(result.stored).toHaveLength(2);
			for (const stored of result.stored) {
				expect(stored).toMatchObject({
					byte_count: content.byteLength,
					content: Buffer.from(content),
					content_hash: identity(content).content_hash,
					state: "available",
					thread_id,
				});
			}
		} finally {
			await runtime.dispose();
		}
	});

	it("retries Stage after a surfaced SQLite writer lock releases", async () => {
		const database_path = await make_database_path();
		const retry_probe = await make_retry_probe();
		const first_runtime = make_runtime(database_path);
		const second_runtime = make_runtime(database_path, retry_probe);
		const thread_id = "thread_snapshot_writer_lock";
		const change_id = "change_snapshot_writer_lock";
		const content = new TextEncoder().encode("writer contention bytes");
		let lock: Awaited<ReturnType<typeof hold_sqlite_write_lock>> | undefined;

		try {
			await first_runtime.runPromise(SeedThread(thread_id));
			await first_runtime.runPromise(SeedReplaceClaim(change_id, thread_id, content));
			await second_runtime.runPromise(
				Effect.service(Database).pipe(
					Effect.flatMap((database) => database.client.run("PRAGMA busy_timeout = 0")),
				),
			);
			lock = await hold_sqlite_write_lock(database_path);
			let stage_settled = false;
			const stage = second_runtime
				.runPromise(
					Effect.service(WorkspaceSnapshotStore).pipe(
						Effect.flatMap((snapshots) =>
							snapshots.Stage(stage_input(change_id, thread_id, content)),
						),
					),
				)
				.then(
					(value) => ({ status: "success" as const, value }),
					(error) => ({ error, status: "failure" as const }),
				)
				.finally(() => {
					stage_settled = true;
				});

			await wait_for_second_attempt(retry_probe);

			expect(stage_settled).toBe(false);
			await lock.Release();
			lock = undefined;
			await continue_second_attempt(retry_probe);
			expect(await within_timeout(stage)).toEqual({
				status: "success",
				value: { status: "staged" },
			});
			const stored = await first_runtime.runPromise(
				Effect.service(Database).pipe(
					Effect.flatMap((database) =>
						database.client.select().from(WorkspaceChangeSnapshots),
					),
				),
			);

			expect(stored).toMatchObject([
				{
					change_id,
					content: Buffer.from(content),
					state: "available",
					thread_id,
				},
			]);
		} finally {
			await lock?.Release();
			await continue_second_attempt(retry_probe);
			await Promise.all([first_runtime.dispose(), second_runtime.dispose()]);
		}
	}, 10_000);

	it("rechecks Stage thread liveness after SQLite contention", async () => {
		const database_path = await make_database_path();
		const retry_probe = await make_retry_probe();
		const first_runtime = make_runtime(database_path);
		const second_runtime = make_runtime(database_path, retry_probe);
		const thread_id = "thread_snapshot_stage_erasure_retry";
		const change_id = "change_snapshot_stage_erasure_retry";
		const content = new TextEncoder().encode("erasure during contention bytes");
		let lock: Awaited<ReturnType<typeof hold_sqlite_write_lock>> | undefined;

		try {
			await first_runtime.runPromise(SeedThread(thread_id));
			await first_runtime.runPromise(SeedReplaceClaim(change_id, thread_id, content));
			await second_runtime.runPromise(
				Effect.service(Database).pipe(
					Effect.flatMap((database) => database.client.run("PRAGMA busy_timeout = 0")),
				),
			);
			lock = await hold_sqlite_write_lock(database_path);
			let stage_settled = false;
			const stage = second_runtime
				.runPromise(
					Effect.service(WorkspaceSnapshotStore).pipe(
						Effect.flatMap((snapshots) =>
							snapshots.Stage(stage_input(change_id, thread_id, content)),
						),
					),
				)
				.then(
					(value) => ({ status: "success" as const, value }),
					(error) => ({ error, status: "failure" as const }),
				)
				.finally(() => {
					stage_settled = true;
				});

			await wait_for_second_attempt(retry_probe);

			expect(stage_settled).toBe(false);
			await lock.Release({
				claimed_at: "2026-07-11T20:00:01.000Z",
				thread_id,
			});
			lock = undefined;
			await continue_second_attempt(retry_probe);
			const stage_result = await within_timeout(stage);

			expect(stage_result).toMatchObject({
				status: "failure",
			});
			expect(JSON.stringify(stage_result)).toContain("WorkspaceSnapshotStoreUnavailable");
			const stored = await first_runtime.runPromise(
				Effect.service(Database).pipe(
					Effect.flatMap((database) =>
						database.client.select().from(WorkspaceChangeSnapshots),
					),
				),
			);

			expect(stored).toEqual([]);
		} finally {
			await lock?.Release();
			await continue_second_attempt(retry_probe);
			await Promise.all([first_runtime.dispose(), second_runtime.dispose()]);
		}
	}, 10_000);

	it("retries Consume after a surfaced SQLite writer lock releases", async () => {
		const database_path = await make_database_path();
		const retry_probe = await make_retry_probe();
		const first_runtime = make_runtime(database_path);
		const second_runtime = make_runtime(database_path, retry_probe);
		const thread_id = "thread_snapshot_consume_writer_lock";
		const change_id = "change_snapshot_consume_writer_lock";
		const content = new TextEncoder().encode("private consume contention bytes");
		let lock: Awaited<ReturnType<typeof hold_sqlite_write_lock>> | undefined;

		try {
			await first_runtime.runPromise(SeedThread(thread_id));
			await first_runtime.runPromise(SeedReplaceClaim(change_id, thread_id, content));
			await first_runtime.runPromise(
				Effect.gen(function* () {
					const snapshots = yield* WorkspaceSnapshotStore;

					yield* snapshots.Stage(stage_input(change_id, thread_id, content));
					yield* CommitReplace(change_id, thread_id, content);
					yield* SeedRollbackOperation(change_id, thread_id, "applied");
				}),
			);
			await second_runtime.runPromise(
				Effect.service(Database).pipe(
					Effect.flatMap((database) => database.client.run("PRAGMA busy_timeout = 0")),
				),
			);
			lock = await hold_sqlite_write_lock(database_path);
			let consume_settled = false;
			const consume = second_runtime
				.runPromise(
					Effect.service(WorkspaceSnapshotStore).pipe(
						Effect.flatMap((snapshots) =>
							snapshots.Consume(consume_input(change_id, thread_id)),
						),
					),
				)
				.then(
					() => ({ status: "success" as const }),
					(error) => ({ error, status: "failure" as const }),
				)
				.finally(() => {
					consume_settled = true;
				});

			await wait_for_second_attempt(retry_probe);

			expect(consume_settled).toBe(false);
			await lock.Release();
			lock = undefined;
			await continue_second_attempt(retry_probe);
			expect(await within_timeout(consume)).toEqual({ status: "success" });
			const stored = await first_runtime.runPromise(
				Effect.service(Database).pipe(
					Effect.flatMap((database) =>
						database.client.select().from(WorkspaceChangeSnapshots),
					),
				),
			);

			expect(stored).toHaveLength(1);
			expect(stored).toMatchObject([
				{
					byte_count: null,
					change_id,
					content: null,
					content_hash: null,
					state: "consumed",
					thread_id,
				},
			]);
		} finally {
			await lock?.Release();
			await continue_second_attempt(retry_probe);
			await Promise.all([first_runtime.dispose(), second_runtime.dispose()]);
		}
	}, 10_000);

	it("allows exactly one conflicting stage across independent database runtimes", async () => {
		const database_path = await make_database_path();
		const first_runtime = make_runtime(database_path);
		const second_runtime = make_runtime(database_path);
		const thread_id = "thread_snapshot_stage_race";
		const change_id = "change_snapshot_stage_race";
		const first = new TextEncoder().encode("first");
		const second = new TextEncoder().encode("second");

		try {
			await first_runtime.runPromise(SeedThread(thread_id));
			await first_runtime.runPromise(SeedReplaceClaim(change_id, thread_id, first));
			const outcomes = await within_timeout(
				Promise.all([
					first_runtime.runPromiseExit(
						Effect.service(WorkspaceSnapshotStore).pipe(
							Effect.flatMap((snapshots) =>
								snapshots.Stage(stage_input(change_id, thread_id, first)),
							),
						),
					),
					second_runtime.runPromiseExit(
						Effect.service(WorkspaceSnapshotStore).pipe(
							Effect.flatMap((snapshots) =>
								snapshots.Stage(stage_input(change_id, thread_id, second)),
							),
						),
					),
				]),
			);
			const successes = outcomes.filter(Exit.isSuccess);
			const failures = outcomes.filter(Exit.isFailure);

			expect(successes).toHaveLength(1);
			expect(failures).toHaveLength(1);
			expect(JSON.stringify(failures[0])).toContain("WorkspaceSnapshotStoreUnavailable");
			const stored = await first_runtime.runPromise(
				Effect.service(Database).pipe(
					Effect.flatMap((database) =>
						database.client
							.select({ content: WorkspaceChangeSnapshots.content })
							.from(WorkspaceChangeSnapshots),
					),
				),
			);

			expect([Buffer.from(first), Buffer.from(second)]).toContainEqual(stored[0]?.content);
		} finally {
			await Promise.all([first_runtime.dispose(), second_runtime.dispose()]);
		}
	}, 10_000);

	it("fails every cross-thread operation without changing the owner snapshot", async () => {
		const runtime = make_runtime(await make_database_path());
		const thread_id = "thread_snapshot_owner";
		const other_thread_id = "thread_snapshot_other";
		const change_id = "change_snapshot_cross_thread";
		const content = new TextEncoder().encode("owned before bytes");

		try {
			await runtime.runPromise(SeedThread(thread_id));
			await runtime.runPromise(SeedThread(other_thread_id));
			await runtime.runPromise(SeedReplaceClaim(change_id, thread_id, content));
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const snapshots = yield* WorkspaceSnapshotStore;

					yield* snapshots.Stage(stage_input(change_id, thread_id, content));
					const resume = yield* Effect.exit(
						snapshots.Resume({
							change_id,
							expected_identity: identity(content),
							thread_id: other_thread_id,
						}),
					);
					yield* CommitReplace(change_id, thread_id, content);
					yield* SeedRollbackOperation(change_id, thread_id, "applied");

					return {
						consume: yield* Effect.exit(
							snapshots.Consume({
								...consume_input(change_id, thread_id),
								thread_id: other_thread_id,
							}),
						),
						exists: yield* Effect.exit(
							snapshots.Exists({ change_id, thread_id: other_thread_id }),
						),
						owner_exists: yield* snapshots.Exists({ change_id, thread_id }),
						owner_read: yield* snapshots.Read({
							change_id,
							expected_identity: identity(content),
							thread_id,
						}),
						read: yield* Effect.exit(
							snapshots.Read({
								change_id,
								expected_identity: identity(content),
								thread_id: other_thread_id,
							}),
						),
						resume,
						stage: yield* Effect.exit(
							snapshots.Stage({
								...stage_input(change_id, thread_id, content),
								thread_id: other_thread_id,
							}),
						),
					};
				}),
			);

			for (const failure of [
				result.consume,
				result.exists,
				result.read,
				result.resume,
				result.stage,
			]) {
				expect(JSON.stringify(failure)).toContain("WorkspaceSnapshotStoreUnavailable");
			}
			expect(result.owner_exists).toBe(true);
			expect(result.owner_read).toEqual(content);
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects unclaimed Stage and Consume operations without writing tombstones", async () => {
		const runtime = make_runtime(await make_database_path());
		const thread_id = "thread_snapshot_unclaimed";
		const stage_change_id = "change_snapshot_unclaimed_stage";
		const consume_change_id = "change_snapshot_unclaimed_consume";
		const content = new TextEncoder().encode("unclaimed private bytes");

		try {
			await runtime.runPromise(SeedThread(thread_id));
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const snapshots = yield* WorkspaceSnapshotStore;

					return {
						consume: yield* Effect.exit(
							snapshots.Consume(consume_input(consume_change_id, thread_id)),
						),
						stage: yield* Effect.exit(
							snapshots.Stage(stage_input(stage_change_id, thread_id, content)),
						),
						stored: yield* database.client.select().from(WorkspaceChangeSnapshots),
					};
				}),
			);

			for (const failure of [result.consume, result.stage]) {
				expect(JSON.stringify(failure)).toContain("WorkspaceSnapshotStoreUnavailable");
			}
			expect(result.stored).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects self-consistent bytes that disagree with the canonical expected identity", async () => {
		const runtime = make_runtime(await make_database_path());
		const thread_id = "thread_snapshot_canonical_identity";
		const change_id = "change_snapshot_canonical_identity";
		const canonical = new TextEncoder().encode("canonical before bytes");
		const arbitrary = new TextEncoder().encode("self-consistent arbitrary bytes");

		try {
			await runtime.runPromise(SeedThread(thread_id));
			await runtime.runPromise(SeedReplaceClaim(change_id, thread_id, canonical));
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const snapshots = yield* WorkspaceSnapshotStore;

					return {
						stage: yield* Effect.exit(
							snapshots.Stage(stage_input(change_id, thread_id, arbitrary)),
						),
						stored: yield* database.client.select().from(WorkspaceChangeSnapshots),
					};
				}),
			);

			expect(JSON.stringify(result.stage)).toContain("WorkspaceSnapshotStoreUnavailable");
			expect(JSON.stringify(result.stage)).not.toContain("self-consistent arbitrary bytes");
			expect(result.stored).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});

	it("returns invalid failures for null runtime input to every operation", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const snapshots = yield* WorkspaceSnapshotStore;

					return {
						consume: yield* Effect.exit(snapshots.Consume(null as never)),
						discard_rejected_replace: yield* Effect.exit(
							snapshots.DiscardRejectedReplace(null as never),
						),
						exists: yield* Effect.exit(snapshots.Exists(null as never)),
						read: yield* Effect.exit(snapshots.Read(null as never)),
						resume: yield* Effect.exit(snapshots.Resume(null as never)),
						stage: yield* Effect.exit(snapshots.Stage(null as never)),
					};
				}),
			);

			for (const failure of Object.values(result)) {
				expect(failure._tag).toBe("Failure");
				expect(JSON.stringify(failure)).toContain("WorkspaceSnapshotStoreInvalid");
			}
		} finally {
			await runtime.dispose();
		}
	});

	it("fences every observable operation once erasure claims or tombstones the thread", async () => {
		const runtime = make_runtime(await make_database_path());
		const thread_id = "thread_snapshot_erasing";
		const change_id = "change_snapshot_erasing";
		const resume_change_id = "change_snapshot_erasing_resume";
		const content = new TextEncoder().encode("private erasure bytes");

		try {
			await runtime.runPromise(SeedThread(thread_id));
			await runtime.runPromise(SeedReplaceClaim(change_id, thread_id, content));
			await runtime.runPromise(SeedReplaceClaim(resume_change_id, thread_id, content));
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const snapshots = yield* WorkspaceSnapshotStore;
					const input = stage_input(change_id, thread_id, content);
					const resume_input = read_input(resume_change_id, thread_id, content);

					yield* snapshots.Stage(input);
					yield* snapshots.Stage(stage_input(resume_change_id, thread_id, content));
					yield* CommitReplace(change_id, thread_id, content);
					yield* SeedRollbackOperation(change_id, thread_id, "applied");
					yield* database.client.insert(ThreadErasureClaims).values({
						claimed_at: "2026-07-11T20:00:01.000Z",
						thread_id,
					});

					const claimed = {
						consume: yield* Effect.exit(
							snapshots.Consume(consume_input(change_id, thread_id)),
						),
						exists: yield* Effect.exit(snapshots.Exists({ change_id, thread_id })),
						read: yield* Effect.exit(
							snapshots.Read({
								change_id,
								expected_identity: input.expected_identity,
								thread_id,
							}),
						),
						resume: yield* Effect.exit(snapshots.Resume(resume_input)),
						stage: yield* Effect.exit(snapshots.Stage(input)),
					};

					yield* database.client.delete(ThreadErasureClaims);
					yield* database.client.delete(Threads);
					yield* database.client.insert(ThreadTombstones).values({
						deleted_at: "2026-07-11T20:00:02.000Z",
						thread_id,
					});

					return {
						claimed,
						tombstoned: {
							consume: yield* Effect.exit(
								snapshots.Consume(consume_input(change_id, thread_id)),
							),
							exists: yield* Effect.exit(snapshots.Exists({ change_id, thread_id })),
							read: yield* Effect.exit(
								snapshots.Read({
									change_id,
									expected_identity: input.expected_identity,
									thread_id,
								}),
							),
							resume: yield* Effect.exit(snapshots.Resume(resume_input)),
							stage: yield* Effect.exit(snapshots.Stage(input)),
						},
					};
				}),
			);

			for (const failure of [
				...Object.values(result.claimed),
				...Object.values(result.tombstoned),
			]) {
				expect(JSON.stringify(failure)).toContain("WorkspaceSnapshotStoreUnavailable");
			}
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects malformed and oversized input without persisting bytes", async () => {
		const runtime = make_runtime(await make_database_path());
		const thread_id = "thread_snapshot_bounds";
		const malformed_change_id = "change_snapshot_malformed";
		const malformed_content = new Uint8Array();
		const oversized_change_id = "change_snapshot_large";
		const oversized = new Uint8Array(4 * 1024 * 1024 + 1);

		try {
			await runtime.runPromise(SeedThread(thread_id));
			await runtime.runPromise(
				SeedReplaceClaim(malformed_change_id, thread_id, malformed_content),
			);
			await runtime.runPromise(SeedReplaceClaim(oversized_change_id, thread_id, oversized));
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const snapshots = yield* WorkspaceSnapshotStore;

					return {
						malformed: yield* Effect.exit(
							snapshots.Stage({
								...stage_input(malformed_change_id, thread_id, malformed_content),
								thread_id: "",
							}),
						),
						oversized: yield* Effect.exit(
							snapshots.Stage(stage_input(oversized_change_id, thread_id, oversized)),
						),
						stored: yield* database.client.select().from(WorkspaceChangeSnapshots),
					};
				}),
			);

			expect(JSON.stringify(result.malformed)).toContain("WorkspaceSnapshotStoreInvalid");
			expect(JSON.stringify(result.oversized)).toContain("WorkspaceSnapshotStoreInvalid");
			expect(result.stored).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects corrupt metadata or content without exposing private bytes", async () => {
		const runtime = make_runtime(await make_database_path());
		const thread_id = "thread_snapshot_corrupt";
		const change_id = "change_snapshot_corrupt";
		const secret = new TextEncoder().encode("private snapshot body must not leak");

		try {
			await runtime.runPromise(SeedThread(thread_id));
			await runtime.runPromise(SeedReplaceClaim(change_id, thread_id, secret));
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const snapshots = yield* WorkspaceSnapshotStore;
					const canonical_identity = identity(secret);
					const altered_content = Buffer.from(new Uint8Array(secret.byteLength).fill(1));

					yield* snapshots.Stage(stage_input(change_id, thread_id, secret));
					yield* database.client
						.update(WorkspaceChangeSnapshots)
						.set({ content_hash: "0".repeat(64) });
					const resume_metadata_failure = yield* Effect.exit(
						snapshots.Resume({
							change_id,
							expected_identity: canonical_identity,
							thread_id,
						}),
					);
					yield* database.client.update(WorkspaceChangeSnapshots).set({
						content: altered_content,
						content_hash: canonical_identity.content_hash,
					});
					const resume_content_failure = yield* Effect.exit(
						snapshots.Resume({
							change_id,
							expected_identity: canonical_identity,
							thread_id,
						}),
					);
					yield* database.client
						.update(WorkspaceChangeSnapshots)
						.set({ content: Buffer.from(secret), content_hash: "0".repeat(64) });
					yield* CommitReplace(change_id, thread_id, secret);
					const metadata_failure = yield* Effect.exit(
						snapshots.Read({
							change_id,
							expected_identity: canonical_identity,
							thread_id,
						}),
					);
					yield* database.client.update(WorkspaceChangeSnapshots).set({
						content: altered_content,
						content_hash: canonical_identity.content_hash,
					});

					return {
						content_failure: yield* Effect.exit(
							snapshots.Read({
								change_id,
								expected_identity: canonical_identity,
								thread_id,
							}),
						),
						metadata_failure,
						resume_content_failure,
						resume_metadata_failure,
					};
				}),
			);

			const serialized = JSON.stringify(result);

			expect(serialized).toContain("WorkspaceSnapshotStoreUnavailable");
			expect(serialized).not.toContain("private snapshot body must not leak");
			expect(serialized).not.toContain("altered snapshot body");
		} finally {
			await runtime.dispose();
		}
	});
});
