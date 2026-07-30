import { createHash } from "node:crypto";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { NodeCrypto } from "@effect/platform-node-shared";
import { Deferred, Effect, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { make_backend_runtime, ThreadErasure } from "@artisan/backend";

import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import {
	JournalCommands,
	JournalEvents,
	EventStreams,
	ThreadErasureClaims,
	Threads,
	ThreadTombstones,
	WorkspaceChangeOperations,
	WorkspaceChanges,
	WorkspaceMutationPayloads,
} from "../../modules/backend/src/persistence/tables";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/metadata";
import {
	WorkspaceMutationPayloadStore,
	WorkspaceMutationPayloadStoreLive,
	type WorkspaceMutationPayloadResumeInput,
	type WorkspaceMutationPayloadStageInput,
} from "../../modules/backend/src/workspace/mutations/payloads";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];

type MutationAction = "replace" | "rollback";
type MutationLifecycle = "applied" | "claimed" | "committed" | "rejected";

function identity(content: Uint8Array) {
	return {
		algorithm: "sha256" as const,
		byte_count: content.byteLength,
		content_hash: createHash("sha256").update(content).digest("hex"),
	};
}

function bytes(value: string) {
	return new TextEncoder().encode(value);
}

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-mutation-payload-store-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

function make_metadata_layer() {
	return Layer.succeed(RuntimeMetadata, {
		instance_id: "payload_store_test",
		MakeId: (prefix) => Effect.succeed(`${prefix}_payload_store_test`),
		Now: Effect.succeed("2026-07-12T12:45:00.000Z"),
	});
}

function make_runtime(database_path: string) {
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		make_metadata_layer(),
		NodeCrypto.layer,
	);

	return ManagedRuntime.make(
		WorkspaceMutationPayloadStoreLive.pipe(Layer.provideMerge(infrastructure)),
	);
}

function SeedThread(thread_id: string) {
	return Effect.gen(function* () {
		const database = yield* Database;

		yield* database.client.insert(Threads).values({
			created_at: "2026-07-12T12:45:00.000Z",
			thread_id,
			title: thread_id,
			title_source: "initial",
			updated_at: "2026-07-12T12:45:00.000Z",
		});
	});
}

function SeedOperation(options: {
	readonly action: MutationAction;
	readonly change_id?: string;
	readonly expected: Uint8Array;
	readonly lifecycle?: MutationLifecycle;
	readonly message_id: string;
	readonly replacement?: Uint8Array;
	readonly thread_id: string;
}) {
	return Effect.gen(function* () {
		const database = yield* Database;
		const change_id = options.change_id ?? `change_${options.message_id}`;

		yield* database.client.insert(WorkspaceChangeOperations).values({
			action: options.action,
			change_id,
			created_at: "2026-07-12T12:45:00.000Z",
			expected_identity_json: JSON.stringify(identity(options.expected)),
			lifecycle: options.lifecycle ?? "claimed",
			message_id: options.message_id,
			request_fingerprint: createHash("sha256").update(options.message_id).digest("hex"),
			result_identity_json:
				options.action === "replace"
					? JSON.stringify(identity(options.replacement!))
					: null,
			sent_at: "2026-07-12T12:45:00.000Z",
			thread_id: options.thread_id,
			updated_at: "2026-07-12T12:45:00.000Z",
		});
	});
}

function SeedReplaceProjection(options: {
	readonly after: Uint8Array;
	readonly before: Uint8Array;
	readonly change_id: string;
	readonly source_command_id: string;
	readonly thread_id: string;
}) {
	return Effect.gen(function* () {
		const database = yield* Database;

		yield* database.client.insert(WorkspaceChanges).values({
			after_identity_json: JSON.stringify(identity(options.after)),
			agent_id: `agent_${options.change_id}`,
			before_identity_json: JSON.stringify(identity(options.before)),
			change_id: options.change_id,
			created_at: "2026-07-12T12:45:00.000Z",
			path: `src/${options.change_id}.ts`,
			review_state: "needs_review",
			rollback_state: "available",
			run_id: `run_${options.change_id}`,
			source_command_id: options.source_command_id,
			thread_id: options.thread_id,
			updated_at: "2026-07-12T12:45:00.000Z",
			version: 1,
			workspace_id: `workspace_${options.thread_id}`,
		});
	});
}

function SeedRollbackContext(options: {
	readonly lifecycle?: MutationLifecycle;
	readonly review_state?: "needs_review" | "reviewed" | "rolled_back";
	readonly rollback_state?: "available" | "consumed";
	readonly suffix: string;
	readonly thread_id: string;
}) {
	return Effect.gen(function* () {
		const database = yield* Database;
		const before = bytes(`before:${options.suffix}`);
		const after = bytes(`after:${options.suffix}`);
		const change_id = `change_${options.suffix}`;
		const replace_message_id = `replace_${options.suffix}`;
		const rollback_message_id = `rollback_${options.suffix}`;
		const review_state = options.review_state ?? "needs_review";
		const rollback_state = options.rollback_state ?? "available";

		yield* SeedOperation({
			action: "replace",
			change_id,
			expected: before,
			lifecycle: "committed",
			message_id: replace_message_id,
			replacement: after,
			thread_id: options.thread_id,
		});
		yield* database.client.insert(WorkspaceChanges).values({
			after_identity_json: JSON.stringify(identity(after)),
			agent_id: `agent_${options.suffix}`,
			before_identity_json: JSON.stringify(identity(before)),
			change_id,
			created_at: "2026-07-12T12:45:00.000Z",
			path: `src/${options.suffix}.ts`,
			review_state,
			rollback_state,
			run_id: `run_${options.suffix}`,
			source_command_id: replace_message_id,
			thread_id: options.thread_id,
			updated_at: "2026-07-12T12:45:00.000Z",
			version: review_state === "needs_review" ? 1 : review_state === "reviewed" ? 2 : 3,
			workspace_id: `workspace_${options.suffix}`,
		});
		yield* SeedOperation({
			action: "rollback",
			change_id,
			expected: after,
			...(options.lifecycle === undefined ? {} : { lifecycle: options.lifecycle }),
			message_id: rollback_message_id,
			thread_id: options.thread_id,
		});

		return {
			after,
			before,
			change_id,
			replace_message_id,
			rollback_message_id,
		};
	});
}

function stage_input(
	action: MutationAction,
	message_id: string,
	thread_id: string,
	expected: Uint8Array,
	replacement: Uint8Array,
): WorkspaceMutationPayloadStageInput {
	return {
		action,
		expected,
		expected_identity: identity(expected),
		message_id,
		replacement,
		replacement_identity: identity(replacement),
		thread_id,
	};
}

function resume_input(
	input: WorkspaceMutationPayloadStageInput,
): WorkspaceMutationPayloadResumeInput {
	return {
		action: input.action,
		expected_identity: input.expected_identity,
		message_id: input.message_id,
		replacement_identity: input.replacement_identity,
		thread_id: input.thread_id,
	};
}

function UpdateLifecycle(message_id: string, lifecycle: MutationLifecycle) {
	return Effect.gen(function* () {
		const database = yield* Database;

		yield* database.client.run(
			`UPDATE workspace_change_operations SET lifecycle = '${lifecycle}' WHERE message_id = '${message_id}'`,
		);
	});
}

function expect_failure_tag(exit: unknown, tag: string) {
	expect(JSON.stringify(exit)).toContain(tag);
}

const concurrent_consume_cases = (["replace", "rollback"] as const).flatMap((action) =>
	Array.from({ length: 10 }, (_, iteration) => [action, iteration] as const),
);

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("WorkspaceMutationPayloadStore", () => {
	it("distinguishes an absent payload from an existing corrupt record without reading bytes", async () => {
		const runtime = make_runtime(await make_database_path());
		const expected = bytes("presence before private source");
		const replacement = bytes("presence after private source");
		const input = stage_input(
			"replace",
			"replace_payload_presence",
			"thread_payload_presence",
			expected,
			replacement,
		);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const store = yield* WorkspaceMutationPayloadStore;

					yield* SeedThread(input.thread_id);
					yield* SeedOperation({
						action: "replace",
						expected,
						message_id: input.message_id,
						replacement,
						thread_id: input.thread_id,
					});
					const absent = yield* store.HasRecord(resume_input(input));

					yield* store.Stage(input);
					yield* database.client
						.update(WorkspaceMutationPayloads)
						.set({ expected_hash: "f".repeat(64) });

					return {
						absent,
						present: yield* store.HasRecord(resume_input(input)),
						resume: yield* store.Resume(resume_input(input)).pipe(Effect.exit),
					};
				}),
			);
			const serialized = JSON.stringify(result);

			expect(result.absent).toBe(false);
			expect(result.present).toBe(true);
			expect_failure_tag(result.resume, "WorkspaceMutationPayloadStoreUnavailable");
			expect(serialized).not.toContain("presence before private source");
			expect(serialized).not.toContain("presence after private source");
		} finally {
			await runtime.dispose();
		}
	});

	it("settles a rejected replace that crashed before private bytes were staged", async () => {
		const runtime = make_runtime(await make_database_path());
		const expected = bytes("unstaged rejected before");
		const replacement = bytes("unstaged rejected after");
		const input = stage_input(
			"replace",
			"replace_rejected_without_payload",
			"thread_rejected_without_payload",
			expected,
			replacement,
		);

		try {
			const rows = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const store = yield* WorkspaceMutationPayloadStore;

					yield* SeedThread(input.thread_id);
					yield* SeedOperation({
						action: "replace",
						expected,
						message_id: input.message_id,
						replacement,
						thread_id: input.thread_id,
					});
					yield* UpdateLifecycle(input.message_id, "rejected");
					yield* store.Consume(resume_input(input));

					return yield* database.client.select().from(WorkspaceMutationPayloads);
				}),
			);

			expect(rows).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});

	it("consumes rejected replace bytes into a tombstone that Stage and Resume cannot resurrect", async () => {
		const runtime = make_runtime(await make_database_path());
		const expected = bytes("rejected replace before");
		const replacement = bytes("rejected replace after");
		const input = stage_input(
			"replace",
			"replace_rejected_cleanup",
			"thread_rejected_replace_cleanup",
			expected,
			replacement,
		);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const store = yield* WorkspaceMutationPayloadStore;

					yield* SeedThread(input.thread_id);
					yield* SeedOperation({
						action: "replace",
						expected,
						message_id: input.message_id,
						replacement,
						thread_id: input.thread_id,
					});
					yield* store.Stage(input);
					yield* UpdateLifecycle(input.message_id, "rejected");
					yield* store.Consume(resume_input(input));
					yield* store.Consume(resume_input(input));

					return {
						resume: yield* store.Resume(resume_input(input)).pipe(Effect.exit),
						row: (yield* database.client.select().from(WorkspaceMutationPayloads))[0],
						stage: yield* store.Stage(input).pipe(Effect.exit),
					};
				}),
			);

			expect_failure_tag(result.resume, "WorkspaceMutationPayloadStoreUnavailable");
			expect_failure_tag(result.stage, "WorkspaceMutationPayloadStoreUnavailable");
			expect(result.row).toMatchObject({
				state: "consumed",
				expected: null,
				expected_byte_count: null,
				expected_hash: null,
				replacement: null,
				replacement_byte_count: null,
				replacement_hash: null,
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("consumes rejected rollback bytes while the unchanged projection remains available", async () => {
		const runtime = make_runtime(await make_database_path());
		const thread_id = "thread_rejected_rollback_cleanup";

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const store = yield* WorkspaceMutationPayloadStore;

					yield* SeedThread(thread_id);
					const context = yield* SeedRollbackContext({
						suffix: "rejected_rollback_cleanup",
						thread_id,
					});
					const input = stage_input(
						"rollback",
						context.rollback_message_id,
						thread_id,
						context.after,
						context.before,
					);

					yield* store.Stage(input);
					yield* UpdateLifecycle(input.message_id, "rejected");
					yield* store.Consume(resume_input(input));

					return {
						projection: (yield* database.client.select().from(WorkspaceChanges))[0],
						row: (yield* database.client.select().from(WorkspaceMutationPayloads))[0],
						resume: yield* store.Resume(resume_input(input)).pipe(Effect.exit),
						stage: yield* store.Stage(input).pipe(Effect.exit),
					};
				}),
			);

			expect(result.projection).toMatchObject({ rollback_state: "available" });
			expect(result.row).toMatchObject({
				state: "consumed",
				expected: null,
				replacement: null,
			});
			expect_failure_tag(result.resume, "WorkspaceMutationPayloadStoreUnavailable");
			expect_failure_tag(result.stage, "WorkspaceMutationPayloadStoreUnavailable");
		} finally {
			await runtime.dispose();
		}
	});

	it("converges two runtimes that consume the same rejected payload", async () => {
		const database_path = await make_database_path();
		const first_runtime = make_runtime(database_path);
		const second_runtime = make_runtime(database_path);
		const expected = bytes("concurrent rejected before");
		const replacement = bytes("concurrent rejected after");
		const input = stage_input(
			"replace",
			"replace_rejected_concurrent",
			"thread_rejected_concurrent",
			expected,
			replacement,
		);

		try {
			await first_runtime.runPromise(
				Effect.gen(function* () {
					const store = yield* WorkspaceMutationPayloadStore;

					yield* SeedThread(input.thread_id);
					yield* SeedOperation({
						action: "replace",
						expected,
						message_id: input.message_id,
						replacement,
						thread_id: input.thread_id,
					});
					yield* store.Stage(input);
					yield* UpdateLifecycle(input.message_id, "rejected");
				}),
			);
			await Promise.all(
				[first_runtime, second_runtime].map((runtime) =>
					runtime.runPromise(
						Effect.service(WorkspaceMutationPayloadStore).pipe(
							Effect.flatMap((store) => store.Consume(resume_input(input))),
						),
					),
				),
			);
			const [row] = await first_runtime.runPromise(
				Effect.service(Database).pipe(
					Effect.flatMap((database) =>
						database.client.select().from(WorkspaceMutationPayloads),
					),
				),
			);

			expect(row).toMatchObject({ state: "consumed", expected: null, replacement: null });
		} finally {
			await Promise.all([first_runtime.dispose(), second_runtime.dispose()]);
		}
	});

	it("rejects a rejected replace payload when a projection aliases its source command", async () => {
		const runtime = make_runtime(await make_database_path());
		const expected = bytes("private rejected alias before");
		const replacement = bytes("private rejected alias after");
		const input = stage_input(
			"replace",
			"replace_rejected_projection_alias",
			"thread_rejected_projection_alias",
			expected,
			replacement,
		);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const store = yield* WorkspaceMutationPayloadStore;

					yield* SeedThread(input.thread_id);
					yield* SeedOperation({
						action: "replace",
						expected,
						message_id: input.message_id,
						replacement,
						thread_id: input.thread_id,
					});
					yield* store.Stage(input);
					yield* UpdateLifecycle(input.message_id, "rejected");
					yield* SeedReplaceProjection({
						after: replacement,
						before: expected,
						change_id: "change_rejected_projection_alias_forged",
						source_command_id: input.message_id,
						thread_id: input.thread_id,
					});

					return {
						exit: yield* store.Consume(resume_input(input)).pipe(Effect.exit),
						row: (yield* database.client.select().from(WorkspaceMutationPayloads))[0],
					};
				}),
			);
			const serialized = JSON.stringify(result.exit);

			expect_failure_tag(result.exit, "WorkspaceMutationPayloadStoreUnavailable");
			expect(serialized).not.toContain(new TextDecoder().decode(expected));
			expect(serialized).not.toContain(new TextDecoder().decode(replacement));
			expect(result.row).toMatchObject({ state: "available" });
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects cleanup of a corrupt rejected payload without exposing bytes", async () => {
		const runtime = make_runtime(await make_database_path());
		const expected = bytes("private corrupt rejected before");
		const replacement = bytes("private corrupt rejected after");
		const input = stage_input(
			"replace",
			"replace_rejected_corrupt_cleanup",
			"thread_rejected_corrupt_cleanup",
			expected,
			replacement,
		);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const store = yield* WorkspaceMutationPayloadStore;

					yield* SeedThread(input.thread_id);
					yield* SeedOperation({
						action: "replace",
						expected,
						message_id: input.message_id,
						replacement,
						thread_id: input.thread_id,
					});
					yield* store.Stage(input);
					yield* UpdateLifecycle(input.message_id, "rejected");
					yield* database.client
						.update(WorkspaceMutationPayloads)
						.set({ expected_hash: "f".repeat(64) });

					return {
						exit: yield* store.Consume(resume_input(input)).pipe(Effect.exit),
						row: (yield* database.client.select().from(WorkspaceMutationPayloads))[0],
					};
				}),
			);
			const serialized = JSON.stringify(result.exit);

			expect_failure_tag(result.exit, "WorkspaceMutationPayloadStoreUnavailable");
			expect(serialized).not.toContain(new TextDecoder().decode(expected));
			expect(serialized).not.toContain(new TextDecoder().decode(replacement));
			expect(result.row).toMatchObject({ state: "available" });
		} finally {
			await runtime.dispose();
		}
	});
	it.each(["claimed", "applied"] as const)(
		"resumes exact replace bytes after restart while the operation is %s",
		async (lifecycle) => {
			const database_path = await make_database_path();
			const first_runtime = make_runtime(database_path);
			const expected = bytes(`before:${lifecycle}`);
			const replacement = bytes(`after:${lifecycle}`);
			const input = stage_input(
				"replace",
				`replace_restart_${lifecycle}`,
				`thread_restart_${lifecycle}`,
				expected,
				replacement,
			);

			try {
				await first_runtime.runPromise(
					Effect.gen(function* () {
						yield* SeedThread(input.thread_id);
						yield* SeedOperation({
							action: "replace",
							expected,
							message_id: input.message_id,
							replacement,
							thread_id: input.thread_id,
						});
						const store = yield* WorkspaceMutationPayloadStore;

						yield* store.Stage(input);

						if (lifecycle === "applied") {
							yield* UpdateLifecycle(input.message_id, "applied");
						}
					}),
				);
			} finally {
				await first_runtime.dispose();
			}

			const second_runtime = make_runtime(database_path);

			try {
				const resumed = await second_runtime.runPromise(
					Effect.gen(function* () {
						const store = yield* WorkspaceMutationPayloadStore;

						return yield* store.Resume(resume_input(input));
					}),
				);

				expect(resumed).toEqual({ expected, replacement });
				expect(resumed.expected).not.toBe(expected);
				expect(resumed.replacement).not.toBe(replacement);
			} finally {
				await second_runtime.dispose();
			}
		},
	);

	it("returns existing for an exact repeated stage", async () => {
		const runtime = make_runtime(await make_database_path());
		const expected = bytes("idempotent before");
		const replacement = bytes("idempotent after");
		const input = stage_input(
			"replace",
			"replace_idempotent",
			"thread_idempotent",
			expected,
			replacement,
		);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedThread(input.thread_id);
					yield* SeedOperation({
						action: "replace",
						expected,
						message_id: input.message_id,
						replacement,
						thread_id: input.thread_id,
					});
					const store = yield* WorkspaceMutationPayloadStore;

					return [yield* store.Stage(input), yield* store.Stage(input)];
				}),
			);

			expect(result).toEqual([{ status: "staged" }, { status: "existing" }]);
		} finally {
			await runtime.dispose();
		}
	});

	it("converges concurrent exact stages across two runtimes", async () => {
		const database_path = await make_database_path();
		const first_runtime = make_runtime(database_path);
		const second_runtime = make_runtime(database_path);
		const expected = bytes("concurrent before");
		const replacement = bytes("concurrent after");
		const input = stage_input(
			"replace",
			"replace_concurrent",
			"thread_concurrent",
			expected,
			replacement,
		);

		try {
			await first_runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedThread(input.thread_id);
					yield* SeedOperation({
						action: "replace",
						expected,
						message_id: input.message_id,
						replacement,
						thread_id: input.thread_id,
					});
				}),
			);

			const stages = await Promise.all([
				first_runtime.runPromise(
					Effect.gen(function* () {
						const store = yield* WorkspaceMutationPayloadStore;

						return yield* store.Stage(input);
					}),
				),
				second_runtime.runPromise(
					Effect.gen(function* () {
						const store = yield* WorkspaceMutationPayloadStore;

						return yield* store.Stage(input);
					}),
				),
			]);

			expect(stages.map((stage) => stage.status).sort()).toEqual(["existing", "staged"]);
		} finally {
			await first_runtime.dispose();
			await second_runtime.dispose();
		}
	});

	it("rejects changed bytes and changed identity before consumption without altering the winner", async () => {
		const runtime = make_runtime(await make_database_path());
		const expected = bytes("winner before");
		const replacement = bytes("winner after");
		const input = stage_input(
			"replace",
			"replace_changed_intent",
			"thread_changed_intent",
			expected,
			replacement,
		);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedThread(input.thread_id);
					yield* SeedOperation({
						action: "replace",
						expected,
						message_id: input.message_id,
						replacement,
						thread_id: input.thread_id,
					});
					const store = yield* WorkspaceMutationPayloadStore;
					const changed = bytes("changed replacement");

					yield* store.Stage(input);

					const changed_bytes = yield* store
						.Stage({ ...input, replacement: changed })
						.pipe(Effect.exit);
					const changed_identity = yield* store
						.Stage({
							...input,
							replacement: changed,
							replacement_identity: identity(changed),
						})
						.pipe(Effect.exit);

					return {
						changed_bytes,
						changed_identity,
						winner: yield* store.Resume(resume_input(input)),
					};
				}),
			);

			expect_failure_tag(result.changed_bytes, "WorkspaceMutationPayloadStoreInvalid");
			expect_failure_tag(result.changed_identity, "WorkspaceMutationPayloadStoreUnavailable");
			expect(result.winner).toEqual({ expected, replacement });
		} finally {
			await runtime.dispose();
		}
	});

	it("does not create a row when an applied operation has no staged payload", async () => {
		const runtime = make_runtime(await make_database_path());
		const expected = bytes("applied missing before");
		const replacement = bytes("applied missing after");
		const input = stage_input(
			"replace",
			"replace_applied_missing",
			"thread_applied_missing",
			expected,
			replacement,
		);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					yield* SeedThread(input.thread_id);
					yield* SeedOperation({
						action: "replace",
						expected,
						lifecycle: "applied",
						message_id: input.message_id,
						replacement,
						thread_id: input.thread_id,
					});
					const store = yield* WorkspaceMutationPayloadStore;

					return {
						rows: yield* database.client.select().from(WorkspaceMutationPayloads),
						stage: yield* store.Stage(input).pipe(Effect.exit),
					};
				}),
			);

			expect_failure_tag(result.stage, "WorkspaceMutationPayloadStoreUnavailable");
			expect(result.rows).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});

	it("consumes a committed replace exactly once and cannot resurrect it", async () => {
		const runtime = make_runtime(await make_database_path());
		const expected = bytes("consume replace before");
		const replacement = bytes("consume replace after");
		const input = stage_input(
			"replace",
			"replace_consume",
			"thread_consume",
			expected,
			replacement,
		);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					yield* SeedThread(input.thread_id);
					yield* SeedOperation({
						action: "replace",
						expected,
						message_id: input.message_id,
						replacement,
						thread_id: input.thread_id,
					});
					const store = yield* WorkspaceMutationPayloadStore;

					yield* store.Stage(input);
					yield* UpdateLifecycle(input.message_id, "committed");

					const first = yield* store.Consume(resume_input(input));
					const duplicate = yield* store.Consume(resume_input(input));
					const [row] = yield* database.client.select().from(WorkspaceMutationPayloads);

					return {
						duplicate,
						first,
						resume: yield* store.Resume(resume_input(input)).pipe(Effect.exit),
						row,
						stage: yield* store.Stage(input).pipe(Effect.exit),
					};
				}),
			);

			expect(result.first).toBeUndefined();
			expect(result.duplicate).toBeUndefined();
			expect(result.row).toMatchObject({
				expected: null,
				expected_byte_count: null,
				expected_hash: null,
				replacement: null,
				replacement_byte_count: null,
				replacement_hash: null,
				state: "consumed",
			});
			expect_failure_tag(result.resume, "WorkspaceMutationPayloadStoreUnavailable");
			expect_failure_tag(result.stage, "WorkspaceMutationPayloadStoreUnavailable");
		} finally {
			await runtime.dispose();
		}
	});

	it.each(["claimed", "applied"] as const)(
		"resumes rollback bytes while the operation is %s",
		async (lifecycle) => {
			const runtime = make_runtime(await make_database_path());

			try {
				const result = await runtime.runPromise(
					Effect.gen(function* () {
						const thread_id = `thread_rollback_${lifecycle}`;

						yield* SeedThread(thread_id);
						const context = yield* SeedRollbackContext({
							lifecycle,
							suffix: `resume_${lifecycle}`,
							thread_id,
						});
						const input = stage_input(
							"rollback",
							context.rollback_message_id,
							thread_id,
							context.after,
							context.before,
						);
						const store = yield* WorkspaceMutationPayloadStore;

						if (lifecycle === "applied") {
							yield* UpdateLifecycle(context.rollback_message_id, "claimed");
						}

						yield* store.Stage(input);

						if (lifecycle === "applied") {
							yield* UpdateLifecycle(context.rollback_message_id, "applied");
						}

						return yield* store.Resume(resume_input(input));
					}),
				);

				expect(result.expected).toEqual(bytes(`after:resume_${lifecycle}`));
				expect(result.replacement).toEqual(bytes(`before:resume_${lifecycle}`));
			} finally {
				await runtime.dispose();
			}
		},
	);

	it("consumes a committed rollback with a rolled-back consumed projection", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const thread_id = "thread_rollback_consume";

					yield* SeedThread(thread_id);
					const context = yield* SeedRollbackContext({
						suffix: "consume_rollback",
						thread_id,
					});
					const input = stage_input(
						"rollback",
						context.rollback_message_id,
						thread_id,
						context.after,
						context.before,
					);
					const store = yield* WorkspaceMutationPayloadStore;

					yield* store.Stage(input);
					yield* UpdateLifecycle(context.rollback_message_id, "committed");
					yield* database.client.update(WorkspaceChanges).set({
						review_state: "rolled_back",
						rollback_state: "consumed",
					});

					const first = yield* store.Consume(resume_input(input));
					const duplicate = yield* store.Consume(resume_input(input));
					const [row] = yield* database.client.select().from(WorkspaceMutationPayloads);

					return { duplicate, first, row };
				}),
			);

			expect(result.first).toBeUndefined();
			expect(result.duplicate).toBeUndefined();
			expect(result.row).toMatchObject({
				expected: null,
				replacement: null,
				state: "consumed",
			});
		} finally {
			await runtime.dispose();
		}
	});

	it.each(concurrent_consume_cases)(
		"concurrently consumes one committed %s payload across two runtimes, iteration %i",
		async (action, iteration) => {
			const database_path = await make_database_path();
			const first_runtime = make_runtime(database_path);
			const second_runtime = make_runtime(database_path);
			const suffix = `concurrent_consume_${action}_${iteration}`;
			const thread_id = `thread_${suffix}`;
			const expected =
				action === "replace" ? bytes(`replace before:${suffix}`) : bytes(`after:${suffix}`);
			const replacement =
				action === "replace" ? bytes(`replace after:${suffix}`) : bytes(`before:${suffix}`);
			const message_id = `${action}_${suffix}`;
			const input = stage_input(action, message_id, thread_id, expected, replacement);

			try {
				await first_runtime.runPromise(
					Effect.gen(function* () {
						const database = yield* Database;

						yield* SeedThread(thread_id);

						if (action === "replace") {
							yield* SeedOperation({
								action,
								expected,
								message_id,
								replacement,
								thread_id,
							});
						} else {
							yield* SeedRollbackContext({ suffix, thread_id });
						}

						const store = yield* WorkspaceMutationPayloadStore;

						yield* store.Stage(input);
						yield* UpdateLifecycle(message_id, "committed");

						if (action === "rollback") {
							yield* database.client.update(WorkspaceChanges).set({
								review_state: "rolled_back",
								rollback_state: "consumed",
							});
						}
					}),
				);

				const start = await Effect.runPromise(Deferred.make<void>());
				const first_ready = await Effect.runPromise(Deferred.make<void>());
				const second_ready = await Effect.runPromise(Deferred.make<void>());
				const first_consume = first_runtime.runPromise(
					Effect.gen(function* () {
						const store = yield* WorkspaceMutationPayloadStore;

						yield* Deferred.succeed(first_ready, undefined);
						yield* Deferred.await(start);

						return yield* store.Consume(resume_input(input));
					}),
				);
				const second_consume = second_runtime.runPromise(
					Effect.gen(function* () {
						const store = yield* WorkspaceMutationPayloadStore;

						yield* Deferred.succeed(second_ready, undefined);
						yield* Deferred.await(start);

						return yield* store.Consume(resume_input(input));
					}),
				);

				await Effect.runPromise(
					Effect.all([Deferred.await(first_ready), Deferred.await(second_ready)], {
						concurrency: "unbounded",
						discard: true,
					}),
				);
				await Effect.runPromise(Deferred.succeed(start, undefined));

				const consumes = await Promise.all([first_consume, second_consume]);
				const rows = await first_runtime.runPromise(
					Effect.gen(function* () {
						const database = yield* Database;

						return yield* database.client.select().from(WorkspaceMutationPayloads);
					}),
				);
				const recovery = await Promise.all(
					[first_runtime, second_runtime].map((runtime) =>
						runtime.runPromise(
							Effect.gen(function* () {
								const store = yield* WorkspaceMutationPayloadStore;

								return {
									resume: yield* store
										.Resume(resume_input(input))
										.pipe(Effect.exit),
									stage: yield* store.Stage(input).pipe(Effect.exit),
								};
							}),
						),
					),
				);
				const rows_after_recovery = await second_runtime.runPromise(
					Effect.gen(function* () {
						const database = yield* Database;

						return yield* database.client.select().from(WorkspaceMutationPayloads);
					}),
				);

				expect(consumes).toEqual([undefined, undefined]);
				expect(rows).toHaveLength(1);
				expect(rows[0]).toMatchObject({
					expected: null,
					expected_byte_count: null,
					expected_hash: null,
					replacement: null,
					replacement_byte_count: null,
					replacement_hash: null,
					state: "consumed",
				});

				for (const attempt of recovery) {
					expect_failure_tag(attempt.resume, "WorkspaceMutationPayloadStoreUnavailable");
					expect_failure_tag(attempt.stage, "WorkspaceMutationPayloadStoreUnavailable");
				}

				expect(rows_after_recovery).toEqual(rows);
			} finally {
				await first_runtime.dispose();
				await second_runtime.dispose();
			}
		},
	);

	it.each(["source_result", "projection_after"] as const)(
		"fails closed when rollback binding has corrupt %s identity",
		async (corruption) => {
			const runtime = make_runtime(await make_database_path());

			try {
				const result = await runtime.runPromise(
					Effect.gen(function* () {
						const database = yield* Database;
						const thread_id = `thread_binding_${corruption}`;

						yield* SeedThread(thread_id);
						const context = yield* SeedRollbackContext({
							suffix: `binding_${corruption}`,
							thread_id,
						});
						const input = stage_input(
							"rollback",
							context.rollback_message_id,
							thread_id,
							context.after,
							context.before,
						);
						const store = yield* WorkspaceMutationPayloadStore;

						yield* store.Stage(input);

						const corrupt_identity = JSON.stringify(identity(bytes("corrupt after")));

						if (corruption === "source_result") {
							yield* database.client.run(
								`UPDATE workspace_change_operations SET result_identity_json = '${corrupt_identity}' WHERE message_id = '${context.replace_message_id}'`,
							);
						} else {
							yield* database.client
								.update(WorkspaceChanges)
								.set({ after_identity_json: corrupt_identity });
						}

						return yield* store.Resume(resume_input(input)).pipe(Effect.exit);
					}),
				);

				expect_failure_tag(result, "WorkspaceMutationPayloadStoreUnavailable");
			} finally {
				await runtime.dispose();
			}
		},
	);

	it("rejects a wrong live thread", async () => {
		const runtime = make_runtime(await make_database_path());
		const expected = bytes("wrong thread before");
		const replacement = bytes("wrong thread after");
		const input = stage_input(
			"replace",
			"replace_wrong_thread",
			"thread_right",
			expected,
			replacement,
		);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedThread("thread_right");
					yield* SeedThread("thread_wrong");
					yield* SeedOperation({
						action: "replace",
						expected,
						message_id: input.message_id,
						replacement,
						thread_id: input.thread_id,
					});
					const store = yield* WorkspaceMutationPayloadStore;

					yield* store.Stage(input);

					return yield* store
						.Resume({ ...resume_input(input), thread_id: "thread_wrong" })
						.pipe(Effect.exit);
				}),
			);

			expect_failure_tag(result, "WorkspaceMutationPayloadStoreUnavailable");
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects the wrong operation action", async () => {
		const runtime = make_runtime(await make_database_path());
		const expected = bytes("wrong action before");
		const replacement = bytes("wrong action after");
		const input = stage_input(
			"replace",
			"replace_wrong_action",
			"thread_wrong_action",
			expected,
			replacement,
		);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedThread(input.thread_id);
					yield* SeedOperation({
						action: "replace",
						expected,
						message_id: input.message_id,
						replacement,
						thread_id: input.thread_id,
					});
					const store = yield* WorkspaceMutationPayloadStore;

					yield* store.Stage(input);

					return yield* store
						.Resume({ ...resume_input(input), action: "rollback" })
						.pipe(Effect.exit);
				}),
			);

			expect_failure_tag(result, "WorkspaceMutationPayloadStoreUnavailable");
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects the wrong expected or replacement identity", async () => {
		const runtime = make_runtime(await make_database_path());
		const expected = bytes("wrong identity before");
		const replacement = bytes("wrong identity after");
		const input = stage_input(
			"replace",
			"replace_wrong_identity",
			"thread_wrong_identity",
			expected,
			replacement,
		);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedThread(input.thread_id);
					yield* SeedOperation({
						action: "replace",
						expected,
						message_id: input.message_id,
						replacement,
						thread_id: input.thread_id,
					});
					const store = yield* WorkspaceMutationPayloadStore;

					yield* store.Stage(input);

					return {
						expected: yield* store
							.Resume({
								...resume_input(input),
								expected_identity: identity(bytes("different expected")),
							})
							.pipe(Effect.exit),
						replacement: yield* store
							.Resume({
								...resume_input(input),
								replacement_identity: identity(bytes("different replacement")),
							})
							.pipe(Effect.exit),
					};
				}),
			);

			expect_failure_tag(result.expected, "WorkspaceMutationPayloadStoreUnavailable");
			expect_failure_tag(result.replacement, "WorkspaceMutationPayloadStoreUnavailable");
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects Resume when the canonical operation has no payload row", async () => {
		const runtime = make_runtime(await make_database_path());
		const expected = bytes("missing row before");
		const replacement = bytes("missing row after");
		const input = stage_input(
			"replace",
			"replace_missing_row",
			"thread_missing_row",
			expected,
			replacement,
		);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedThread(input.thread_id);
					yield* SeedOperation({
						action: "replace",
						expected,
						message_id: input.message_id,
						replacement,
						thread_id: input.thread_id,
					});
					const store = yield* WorkspaceMutationPayloadStore;

					return yield* store.Resume(resume_input(input)).pipe(Effect.exit);
				}),
			);

			expect_failure_tag(result, "WorkspaceMutationPayloadStoreUnavailable");
		} finally {
			await runtime.dispose();
		}
	});

	it.each(["blob", "hash", "count"] as const)(
		"fails closed when the private expected %s is directly corrupted",
		async (corruption) => {
			const runtime = make_runtime(await make_database_path());
			const expected = bytes(`corrupt ${corruption} before`);
			const replacement = bytes(`corrupt ${corruption} after`);
			const input = stage_input(
				"replace",
				`replace_corrupt_${corruption}`,
				`thread_corrupt_${corruption}`,
				expected,
				replacement,
			);

			try {
				const result = await runtime.runPromise(
					Effect.gen(function* () {
						const database = yield* Database;

						yield* SeedThread(input.thread_id);
						yield* SeedOperation({
							action: "replace",
							expected,
							message_id: input.message_id,
							replacement,
							thread_id: input.thread_id,
						});
						const store = yield* WorkspaceMutationPayloadStore;

						yield* store.Stage(input);
						yield* database.client.run("PRAGMA ignore_check_constraints = ON");

						if (corruption === "blob") {
							yield* database.client
								.update(WorkspaceMutationPayloads)
								.set({ expected: Buffer.from("corrupt") });
						} else if (corruption === "hash") {
							yield* database.client
								.update(WorkspaceMutationPayloads)
								.set({ expected_hash: "f".repeat(64) });
						} else {
							yield* database.client
								.update(WorkspaceMutationPayloads)
								.set({ expected_byte_count: expected.byteLength + 1 });
						}

						yield* database.client.run("PRAGMA ignore_check_constraints = OFF");

						return yield* store.Resume(resume_input(input)).pipe(Effect.exit);
					}),
				);

				expect_failure_tag(result, "WorkspaceMutationPayloadStoreUnavailable");
			} finally {
				await runtime.dispose();
			}
		},
	);

	it("rejects Resume after a thread erasure claim", async () => {
		const runtime = make_runtime(await make_database_path());
		const expected = bytes("erasure claim before");
		const replacement = bytes("erasure claim after");
		const input = stage_input(
			"replace",
			"replace_erasure_claim",
			"thread_erasure_claim",
			expected,
			replacement,
		);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					yield* SeedThread(input.thread_id);
					yield* SeedOperation({
						action: "replace",
						expected,
						message_id: input.message_id,
						replacement,
						thread_id: input.thread_id,
					});
					const store = yield* WorkspaceMutationPayloadStore;

					yield* store.Stage(input);
					yield* database.client.insert(ThreadErasureClaims).values({
						claimed_at: "2026-07-12T12:45:01.000Z",
						thread_id: input.thread_id,
					});

					return yield* store.Resume(resume_input(input)).pipe(Effect.exit);
				}),
			);

			expect_failure_tag(result, "WorkspaceMutationPayloadStoreUnavailable");
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects Resume after a thread tombstone", async () => {
		const runtime = make_runtime(await make_database_path());
		const expected = bytes("tombstone before");
		const replacement = bytes("tombstone after");
		const input = stage_input(
			"replace",
			"replace_tombstone",
			"thread_tombstone",
			expected,
			replacement,
		);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					yield* SeedThread(input.thread_id);
					yield* SeedOperation({
						action: "replace",
						expected,
						message_id: input.message_id,
						replacement,
						thread_id: input.thread_id,
					});
					const store = yield* WorkspaceMutationPayloadStore;

					yield* store.Stage(input);
					yield* database.client.insert(ThreadTombstones).values({
						deleted_at: "2026-07-12T12:45:01.000Z",
						thread_id: input.thread_id,
					});

					return yield* store.Resume(resume_input(input)).pipe(Effect.exit);
				}),
			);

			expect_failure_tag(result, "WorkspaceMutationPayloadStoreUnavailable");
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects malformed input before storage", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const store = yield* WorkspaceMutationPayloadStore;

					return yield* store.Stage({} as never).pipe(Effect.exit);
				}),
			);

			expect_failure_tag(result, "WorkspaceMutationPayloadStoreInvalid");
		} finally {
			await runtime.dispose();
		}
	});

	it("preserves pending payload rows when real ThreadErasure releases its claim", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			runtime_metadata: make_metadata_layer(),
		});
		const thread_id = "thread_real_erasure";
		const expected = bytes("real erasure before");
		const replacement = bytes("real erasure after");
		const input = stage_input(
			"replace",
			"replace_real_erasure",
			thread_id,
			expected,
			replacement,
		);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const erasure = yield* ThreadErasure;
					const store = yield* WorkspaceMutationPayloadStore;

					yield* SeedThread(thread_id);
					yield* database.client.insert(EventStreams).values({
						last_sequence: 0,
						stream_id: `thread:${thread_id}`,
					});
					yield* SeedOperation({
						action: "replace",
						expected,
						message_id: input.message_id,
						replacement,
						thread_id,
					});
					yield* store.Stage(input);

					const before = yield* database.client.select().from(WorkspaceMutationPayloads);

					yield* database.client.insert(ThreadErasureClaims).values({
						claimed_at: "2026-07-12T12:45:01.000Z",
						thread_id,
					});

					const erased = yield* erasure.ResumeClaimed("2026-07-12T12:45:02.000Z");

					return {
						after_operations: yield* database.client
							.select()
							.from(WorkspaceChangeOperations),
						after_payloads: yield* database.client
							.select()
							.from(WorkspaceMutationPayloads),
						claims: yield* database.client.select().from(ThreadErasureClaims),
						threads: yield* database.client.select().from(Threads),
						before,
						erased,
					};
				}),
			);

			expect(result.before).toHaveLength(1);
			expect(result.erased).toEqual([]);
			expect(result.claims).toEqual([]);
			expect(result.after_payloads).toHaveLength(1);
			expect(result.after_operations).toHaveLength(1);
			expect(result.threads).toHaveLength(1);
		} finally {
			await runtime.dispose();
		}
	});

	it("keeps payload source bytes out of commands, events, and operations", async () => {
		const database_path = await make_database_path();
		const runtime = make_runtime(database_path);
		const thread_id = "thread_source_free";
		const expected = bytes("private expected source text");
		const replacement = bytes("private replacement source text");
		const input = stage_input(
			"replace",
			"replace_source_free",
			thread_id,
			expected,
			replacement,
		);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const store = yield* WorkspaceMutationPayloadStore;

					yield* SeedThread(thread_id);
					yield* database.client.insert(JournalCommands).values({
						accepted_at: "2026-07-12T12:45:00.000Z",
						message_id: "content_free_command",
						origin: "frontend",
						payload_json: '{"type":"thread.create"}',
						payload_type: "thread.create",
						schema_version: 1,
						sent_at: "2026-07-12T12:45:00.000Z",
						status: "accepted",
						thread_id,
					});
					yield* database.client.insert(JournalEvents).values({
						causation_id: "content_free_command",
						correlation_id: "content_free_command",
						event_id: "content_free_event",
						event_type: "thread.created",
						occurred_at: "2026-07-12T12:45:00.000Z",
						origin: "backend",
						payload_json: '{"type":"thread.created"}',
						schema_version: 1,
						stream_id: `thread:${thread_id}`,
						stream_sequence: 1,
						thread_id,
					});
					yield* SeedOperation({
						action: "replace",
						expected,
						message_id: input.message_id,
						replacement,
						thread_id,
					});
					yield* store.Stage(input);

					return {
						commands: yield* database.client.select().from(JournalCommands),
						events: yield* database.client.select().from(JournalEvents),
						operations: yield* database.client.select().from(WorkspaceChangeOperations),
						payloads: yield* database.client.select().from(WorkspaceMutationPayloads),
					};
				}),
			);
			const public_persistence = JSON.stringify({
				commands: result.commands,
				events: result.events,
				operations: result.operations,
			});

			expect(result.commands.length).toBeGreaterThan(0);
			expect(result.events.length).toBeGreaterThan(0);
			expect(result.operations).toHaveLength(1);
			expect(public_persistence).not.toContain("private expected source text");
			expect(public_persistence).not.toContain("private replacement source text");
			expect(new Uint8Array(result.payloads[0]!.expected!)).toEqual(expected);
			expect(new Uint8Array(result.payloads[0]!.replacement!)).toEqual(replacement);
		} finally {
			await runtime.dispose();
		}
	});
});
