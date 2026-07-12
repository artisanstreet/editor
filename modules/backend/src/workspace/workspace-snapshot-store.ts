import { Context, Crypto, Data, Effect, Encoding, Layer, Schema } from "effect";
import { and, eq, sql } from "drizzle-orm";

import {
	ContentIdentity,
	Identifier,
	workspace_text_maximum_bytes,
	type ContentIdentity as ContentIdentityValue,
} from "@artisan/protocol";

import { Database } from "../persistence/database";
import { RetrySqliteWrite } from "../persistence/sqlite-write-retry";
import {
	ThreadErasureClaims,
	Threads,
	ThreadTombstones,
	WorkspaceChangeOperations,
	WorkspaceChanges,
	WorkspaceChangeSnapshots,
} from "../persistence/schema";
import { RuntimeMetadata } from "../runtime/runtime-metadata";

const SnapshotBytes = Schema.Uint8Array.check(
	Schema.makeFilter<Uint8Array>((bytes) =>
		bytes.byteLength <= workspace_text_maximum_bytes
			? undefined
			: `Expected at most ${workspace_text_maximum_bytes} bytes`,
	),
);

const StageInput = Schema.Struct({
	change_id: Identifier,
	content: SnapshotBytes,
	expected_identity: ContentIdentity,
	thread_id: Identifier,
});

const ReadInput = Schema.Struct({
	change_id: Identifier,
	expected_identity: ContentIdentity,
	thread_id: Identifier,
});

const ConsumeInput = Schema.Struct({
	change_id: Identifier,
	rollback_message_id: Identifier,
	thread_id: Identifier,
});

const ExistsInput = Schema.Struct({
	change_id: Identifier,
	thread_id: Identifier,
});
const ContentHash = /^[0-9a-f]{64}$/;

type ReplaceLifecycle = "applied" | "claimed" | "committed";

/** Supplies bytes and their declared identity for one staged rollback snapshot. */
export type WorkspaceSnapshotStageInput = typeof StageInput.Type;

/** Supplies the expected identity required to read one rollback snapshot. */
export type WorkspaceSnapshotReadInput = typeof ReadInput.Type;

/** Supplies the thread authority required to permanently consume one snapshot. */
export type WorkspaceSnapshotConsumeInput = typeof ConsumeInput.Type;

/** Supplies the thread authority required to inspect snapshot availability. */
export type WorkspaceSnapshotExistsInput = typeof ExistsInput.Type;

/** Reports malformed snapshot-store input before it reaches private storage. */
export class WorkspaceSnapshotStoreInvalid extends Data.TaggedError(
	"WorkspaceSnapshotStoreInvalid",
)<{
	readonly change_id?: string;
	readonly operation: "consume" | "exists" | "read" | "stage";
}> {}

/** Reports a change ID already bound to a different available snapshot. */
export class WorkspaceSnapshotStoreConflict extends Data.TaggedError(
	"WorkspaceSnapshotStoreConflict",
)<{
	readonly change_id: string;
	readonly operation: "stage";
}> {}

/** Reports a missing, consumed, corrupt, or otherwise unavailable snapshot. */
export class WorkspaceSnapshotStoreUnavailable extends Data.TaggedError(
	"WorkspaceSnapshotStoreUnavailable",
)<{
	readonly change_id?: string;
	readonly operation: "consume" | "exists" | "read" | "stage";
}> {}

/** Represents failures returned by private workspace rollback snapshot operations. */
export type WorkspaceSnapshotStoreError =
	| WorkspaceSnapshotStoreConflict
	| WorkspaceSnapshotStoreInvalid
	| WorkspaceSnapshotStoreUnavailable;

/** Owns opaque private rollback snapshots independently of the workspace journal. */
export class WorkspaceSnapshotStore extends Context.Service<
	WorkspaceSnapshotStore,
	{
		readonly Consume: (
			input: WorkspaceSnapshotConsumeInput,
		) => Effect.Effect<void, WorkspaceSnapshotStoreError>;
		readonly Exists: (
			input: WorkspaceSnapshotExistsInput,
		) => Effect.Effect<boolean, WorkspaceSnapshotStoreError>;
		readonly Read: (
			input: WorkspaceSnapshotReadInput,
		) => Effect.Effect<Uint8Array, WorkspaceSnapshotStoreError>;
		readonly Stage: (
			input: WorkspaceSnapshotStageInput,
		) => Effect.Effect<{ readonly status: "existing" | "staged" }, WorkspaceSnapshotStoreError>;
	}
>()("Artisan/WorkspaceSnapshotStore") {}

function identities_match(left: ContentIdentityValue, right: ContentIdentityValue) {
	return (
		left.algorithm === right.algorithm &&
		left.byte_count === right.byte_count &&
		left.content_hash === right.content_hash
	);
}

function bytes_match(left: Uint8Array, right: Uint8Array) {
	return (
		left.byteLength === right.byteLength && left.every((value, index) => value === right[index])
	);
}

function is_replace_lifecycle(value: string): value is ReplaceLifecycle {
	return value === "applied" || value === "claimed" || value === "committed";
}

function make_unavailable(
	operation: WorkspaceSnapshotStoreUnavailable["operation"],
	change_id?: string,
) {
	return new WorkspaceSnapshotStoreUnavailable({
		...(change_id === undefined ? {} : { change_id }),
		operation,
	});
}

function conceal_error(
	error: unknown,
	operation: WorkspaceSnapshotStoreUnavailable["operation"],
	change_id?: string,
) {
	if (
		error instanceof WorkspaceSnapshotStoreConflict ||
		error instanceof WorkspaceSnapshotStoreInvalid ||
		error instanceof WorkspaceSnapshotStoreUnavailable
	) {
		return error;
	}

	return make_unavailable(operation, change_id);
}

function change_id_from_unknown(input: unknown): string | undefined {
	return typeof input === "object" &&
		input !== null &&
		"change_id" in input &&
		typeof input.change_id === "string"
		? input.change_id
		: undefined;
}

function Decode<A>(
	schema: Schema.Codec<A, A>,
	input: unknown,
	operation: WorkspaceSnapshotStoreInvalid["operation"],
) {
	const change_id = change_id_from_unknown(input);

	return Schema.decodeUnknownEffect(schema, { onExcessProperty: "error" })(input).pipe(
		Effect.mapError(
			() =>
				new WorkspaceSnapshotStoreInvalid({
					...(change_id === undefined ? {} : { change_id }),
					operation,
				}),
		),
	);
}

function DecodeStoredIdentity(
	value: string | null,
	operation: WorkspaceSnapshotStoreUnavailable["operation"],
	change_id: string,
) {
	if (value === null) {
		return Effect.fail(make_unavailable(operation, change_id));
	}

	return Effect.try({
		catch: () => make_unavailable(operation, change_id),
		try: () => JSON.parse(value) as unknown,
	}).pipe(
		Effect.flatMap(Schema.decodeUnknownEffect(ContentIdentity, { onExcessProperty: "error" })),
		Effect.mapError(() => make_unavailable(operation, change_id)),
	);
}

function SnapshotIdentity(crypto: Crypto.Crypto, bytes: Uint8Array) {
	return crypto.digest("SHA-256", bytes).pipe(
		Effect.map((digest) => ({
			algorithm: "sha256" as const,
			byte_count: bytes.byteLength,
			content_hash: Encoding.encodeHex(digest),
		})),
	);
}

/** Builds the SQLite-backed private snapshot store. */
export const WorkspaceSnapshotStoreLive = Layer.effect(
	WorkspaceSnapshotStore,
	Effect.gen(function* () {
		const crypto = yield* Crypto.Crypto;
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;

		const EnsureLiveThread = (
			transaction: typeof database.client,
			thread_id: string,
			operation: WorkspaceSnapshotStoreUnavailable["operation"],
			change_id: string,
		) =>
			Effect.gen(function* () {
				const [thread] = yield* transaction
					.select({ thread_id: Threads.thread_id })
					.from(Threads)
					.where(eq(Threads.thread_id, thread_id))
					.limit(1);
				const [claim] = yield* transaction
					.select({ thread_id: ThreadErasureClaims.thread_id })
					.from(ThreadErasureClaims)
					.where(eq(ThreadErasureClaims.thread_id, thread_id))
					.limit(1);
				const [tombstone] = yield* transaction
					.select({ thread_id: ThreadTombstones.thread_id })
					.from(ThreadTombstones)
					.where(eq(ThreadTombstones.thread_id, thread_id))
					.limit(1);

				if (!thread || claim || tombstone) {
					return yield* Effect.fail(make_unavailable(operation, change_id));
				}
			});

		const EnsureCanonicalReplace = (
			transaction: typeof database.client,
			thread_id: string,
			change_id: string,
			operation: WorkspaceSnapshotStoreUnavailable["operation"],
			expected_identity?: ContentIdentityValue,
		) =>
			Effect.gen(function* () {
				yield* EnsureLiveThread(transaction, thread_id, operation, change_id);

				const [replace] = yield* transaction
					.select({
						expected_identity_json: WorkspaceChangeOperations.expected_identity_json,
						lifecycle: WorkspaceChangeOperations.lifecycle,
						message_id: WorkspaceChangeOperations.message_id,
						thread_id: WorkspaceChangeOperations.thread_id,
					})
					.from(WorkspaceChangeOperations)
					.where(
						and(
							eq(WorkspaceChangeOperations.change_id, change_id),
							eq(WorkspaceChangeOperations.action, "replace"),
						),
					)
					.limit(1);

				if (
					!replace ||
					replace.thread_id !== thread_id ||
					!is_replace_lifecycle(replace.lifecycle)
				) {
					return yield* Effect.fail(make_unavailable(operation, change_id));
				}

				const canonical_identity = yield* DecodeStoredIdentity(
					replace.expected_identity_json,
					operation,
					change_id,
				);

				if (
					expected_identity !== undefined &&
					!identities_match(canonical_identity, expected_identity)
				) {
					return yield* Effect.fail(make_unavailable(operation, change_id));
				}

				const [projection] = yield* transaction
					.select({
						after_identity_json: WorkspaceChanges.after_identity_json,
						before_identity_json: WorkspaceChanges.before_identity_json,
						rollback_state: WorkspaceChanges.rollback_state,
						source_command_id: WorkspaceChanges.source_command_id,
						thread_id: WorkspaceChanges.thread_id,
					})
					.from(WorkspaceChanges)
					.where(eq(WorkspaceChanges.change_id, change_id))
					.limit(1);

				if ((replace.lifecycle === "committed") !== (projection !== undefined)) {
					return yield* Effect.fail(make_unavailable(operation, change_id));
				}

				if (projection) {
					const projected_identity = yield* DecodeStoredIdentity(
						projection.before_identity_json,
						operation,
						change_id,
					);
					const projected_after_identity = yield* DecodeStoredIdentity(
						projection.after_identity_json,
						operation,
						change_id,
					);

					if (
						projection.source_command_id !== replace.message_id ||
						projection.thread_id !== thread_id ||
						(projection.rollback_state !== "available" &&
							projection.rollback_state !== "consumed") ||
						!identities_match(projected_identity, canonical_identity)
					) {
						return yield* Effect.fail(make_unavailable(operation, change_id));
					}

					return {
						before_identity: canonical_identity,
						lifecycle: replace.lifecycle,
						projection: {
							after_identity: projected_after_identity,
							rollback_state: projection.rollback_state,
						},
					};
				}

				return {
					before_identity: canonical_identity,
					lifecycle: replace.lifecycle,
					projection: undefined,
				};
			});

		const EnsureAppliedRollback = (
			transaction: typeof database.client,
			thread_id: string,
			change_id: string,
			rollback_message_id: string,
		) =>
			Effect.gen(function* () {
				const replace = yield* EnsureCanonicalReplace(
					transaction,
					thread_id,
					change_id,
					"consume",
				);

				if (replace.lifecycle !== "committed" || replace.projection === undefined) {
					return yield* Effect.fail(make_unavailable("consume", change_id));
				}

				const [rollback] = yield* transaction
					.select({
						action: WorkspaceChangeOperations.action,
						change_id: WorkspaceChangeOperations.change_id,
						expected_identity_json: WorkspaceChangeOperations.expected_identity_json,
						lifecycle: WorkspaceChangeOperations.lifecycle,
						thread_id: WorkspaceChangeOperations.thread_id,
					})
					.from(WorkspaceChangeOperations)
					.where(eq(WorkspaceChangeOperations.message_id, rollback_message_id))
					.limit(1);

				if (
					!rollback ||
					rollback.action !== "rollback" ||
					rollback.change_id !== change_id ||
					rollback.thread_id !== thread_id ||
					(rollback.lifecycle !== "applied" && rollback.lifecycle !== "committed")
				) {
					return yield* Effect.fail(make_unavailable("consume", change_id));
				}

				const expected_after = yield* DecodeStoredIdentity(
					rollback.expected_identity_json,
					"consume",
					change_id,
				);

				if (!identities_match(expected_after, replace.projection.after_identity)) {
					return yield* Effect.fail(make_unavailable("consume", change_id));
				}

				if (
					(rollback.lifecycle === "applied" &&
						replace.projection.rollback_state !== "available") ||
					(rollback.lifecycle === "committed" &&
						replace.projection.rollback_state !== "consumed")
				) {
					return yield* Effect.fail(make_unavailable("consume", change_id));
				}

				return rollback.lifecycle;
			});

		const Stage = (
			input: WorkspaceSnapshotStageInput,
		): Effect.Effect<
			{ readonly status: "existing" | "staged" },
			WorkspaceSnapshotStoreError
		> => {
			const change_id = change_id_from_unknown(input);

			return Effect.gen(function* () {
				const decoded = yield* Decode(StageInput, input, "stage");
				const actual_identity = yield* SnapshotIdentity(crypto, decoded.content).pipe(
					Effect.mapError(() => make_unavailable("stage", decoded.change_id)),
				);

				if (!identities_match(actual_identity, decoded.expected_identity)) {
					return yield* Effect.fail(
						new WorkspaceSnapshotStoreInvalid({
							change_id: decoded.change_id,
							operation: "stage",
						}),
					);
				}

				const now = yield* metadata.Now;

				return yield* RetrySqliteWrite(
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							const canonical = yield* EnsureCanonicalReplace(
								transaction,
								decoded.thread_id,
								decoded.change_id,
								"stage",
								decoded.expected_identity,
							);

							const [preexisting] = yield* transaction
								.select({ state: WorkspaceChangeSnapshots.state })
								.from(WorkspaceChangeSnapshots)
								.where(eq(WorkspaceChangeSnapshots.change_id, decoded.change_id))
								.limit(1);

							if (canonical.lifecycle === "committed" && !preexisting) {
								return yield* Effect.fail(
									make_unavailable("stage", decoded.change_id),
								);
							}

							const inserted =
								canonical.lifecycle === "committed"
									? []
									: yield* transaction
											.insert(WorkspaceChangeSnapshots)
											.values({
												byte_count: actual_identity.byte_count,
												change_id: decoded.change_id,
												content: Buffer.from(decoded.content),
												content_hash: actual_identity.content_hash,
												created_at: now,
												state: "available",
												thread_id: decoded.thread_id,
												updated_at: now,
											})
											.onConflictDoNothing()
											.returning({
												change_id: WorkspaceChangeSnapshots.change_id,
											});

							const status =
								inserted.length === 1 ? ("staged" as const) : ("existing" as const);

							const [winner] = yield* transaction
								.select({
									byte_count: WorkspaceChangeSnapshots.byte_count,
									content_hash: WorkspaceChangeSnapshots.content_hash,
									state: WorkspaceChangeSnapshots.state,
									thread_id: WorkspaceChangeSnapshots.thread_id,
								})
								.from(WorkspaceChangeSnapshots)
								.where(eq(WorkspaceChangeSnapshots.change_id, decoded.change_id))
								.limit(1);

							if (!winner || winner.state === "consumed") {
								return yield* Effect.fail(
									make_unavailable("stage", decoded.change_id),
								);
							}

							if (winner.thread_id !== decoded.thread_id) {
								return yield* Effect.fail(
									new WorkspaceSnapshotStoreConflict({
										change_id: decoded.change_id,
										operation: "stage",
									}),
								);
							}

							if (
								winner.byte_count !== actual_identity.byte_count ||
								winner.content_hash !== actual_identity.content_hash
							) {
								return yield* Effect.fail(
									new WorkspaceSnapshotStoreConflict({
										change_id: decoded.change_id,
										operation: "stage",
									}),
								);
							}

							const [stored] = yield* transaction
								.select({
									content: WorkspaceChangeSnapshots.content,
								})
								.from(WorkspaceChangeSnapshots)
								.where(eq(WorkspaceChangeSnapshots.change_id, decoded.change_id))
								.limit(1);

							if (!stored?.content || !bytes_match(stored.content, decoded.content)) {
								return yield* Effect.fail(
									new WorkspaceSnapshotStoreConflict({
										change_id: decoded.change_id,
										operation: "stage",
									}),
								);
							}

							const stored_identity = yield* SnapshotIdentity(
								crypto,
								stored.content,
							).pipe(
								Effect.mapError(() => make_unavailable("stage", decoded.change_id)),
							);

							if (!identities_match(stored_identity, actual_identity)) {
								return yield* Effect.fail(
									new WorkspaceSnapshotStoreConflict({
										change_id: decoded.change_id,
										operation: "stage",
									}),
								);
							}

							return { status };
						}),
					),
				);
			}).pipe(Effect.mapError((error) => conceal_error(error, "stage", change_id)));
		};

		const Consume = (
			input: WorkspaceSnapshotConsumeInput,
		): Effect.Effect<void, WorkspaceSnapshotStoreError> => {
			const change_id = change_id_from_unknown(input);

			return Decode(ConsumeInput, input, "consume")
				.pipe(
					Effect.flatMap((decoded) =>
						metadata.Now.pipe(
							Effect.flatMap((now) =>
								RetrySqliteWrite(
									database.client.transaction((transaction) =>
										Effect.gen(function* () {
											yield* EnsureAppliedRollback(
												transaction,
												decoded.thread_id,
												decoded.change_id,
												decoded.rollback_message_id,
											);

											const ReadStoredState = transaction
												.select({
													state: WorkspaceChangeSnapshots.state,
													thread_id: WorkspaceChangeSnapshots.thread_id,
												})
												.from(WorkspaceChangeSnapshots)
												.where(
													eq(
														WorkspaceChangeSnapshots.change_id,
														decoded.change_id,
													),
												)
												.limit(1);
											const [stored] = yield* ReadStoredState;

											if (!stored || stored.thread_id !== decoded.thread_id) {
												return yield* Effect.fail(
													make_unavailable("consume", decoded.change_id),
												);
											}

											if (stored.state === "consumed") {
												return;
											}

											const consumed = yield* transaction
												.update(WorkspaceChangeSnapshots)
												.set({
													byte_count: null,
													content: null,
													content_hash: null,
													state: "consumed",
													updated_at: now,
												})
												.where(
													and(
														eq(
															WorkspaceChangeSnapshots.change_id,
															decoded.change_id,
														),
														eq(
															WorkspaceChangeSnapshots.thread_id,
															decoded.thread_id,
														),
														eq(
															WorkspaceChangeSnapshots.state,
															"available",
														),
													),
												)
												.returning({
													change_id: WorkspaceChangeSnapshots.change_id,
												});

											if (consumed.length !== 1) {
												const [current] = yield* ReadStoredState;

												if (
													current?.thread_id === decoded.thread_id &&
													current.state === "consumed"
												) {
													return;
												}

												return yield* Effect.fail(
													make_unavailable("consume", decoded.change_id),
												);
											}
										}),
									),
								),
							),
						),
					),
				)
				.pipe(Effect.mapError((error) => conceal_error(error, "consume", change_id)));
		};

		const Read = (
			input: WorkspaceSnapshotReadInput,
		): Effect.Effect<Uint8Array, WorkspaceSnapshotStoreError> => {
			const change_id = change_id_from_unknown(input);

			return Decode(ReadInput, input, "read")
				.pipe(
					Effect.flatMap((decoded) =>
						database.client.transaction((transaction) =>
							Effect.gen(function* () {
								const canonical = yield* EnsureCanonicalReplace(
									transaction,
									decoded.thread_id,
									decoded.change_id,
									"read",
									decoded.expected_identity,
								);

								if (
									canonical.lifecycle !== "committed" ||
									canonical.projection?.rollback_state !== "available"
								) {
									return yield* Effect.fail(
										make_unavailable("read", decoded.change_id),
									);
								}

								const [metadata_row] = yield* transaction
									.select({
										byte_count: WorkspaceChangeSnapshots.byte_count,
										content_hash: WorkspaceChangeSnapshots.content_hash,
										content_length: sql<
											number | null
										>`length(${WorkspaceChangeSnapshots.content})`,
										state: WorkspaceChangeSnapshots.state,
										thread_id: WorkspaceChangeSnapshots.thread_id,
									})
									.from(WorkspaceChangeSnapshots)
									.where(
										eq(WorkspaceChangeSnapshots.change_id, decoded.change_id),
									)
									.limit(1);

								if (
									!metadata_row ||
									metadata_row.state !== "available" ||
									typeof metadata_row.byte_count !== "number" ||
									typeof metadata_row.content_length !== "number" ||
									typeof metadata_row.content_hash !== "string" ||
									metadata_row.byte_count < 0 ||
									metadata_row.byte_count > workspace_text_maximum_bytes ||
									metadata_row.content_length !== metadata_row.byte_count ||
									!ContentHash.test(metadata_row.content_hash) ||
									!identities_match(
										{
											algorithm: "sha256",
											byte_count: metadata_row.byte_count,
											content_hash: metadata_row.content_hash,
										},
										decoded.expected_identity,
									)
								) {
									return yield* Effect.fail(
										make_unavailable("read", decoded.change_id),
									);
								}

								if (metadata_row.thread_id !== decoded.thread_id) {
									return yield* Effect.fail(
										make_unavailable("read", decoded.change_id),
									);
								}

								const [content_row] = yield* transaction
									.select({ content: WorkspaceChangeSnapshots.content })
									.from(WorkspaceChangeSnapshots)
									.where(
										eq(WorkspaceChangeSnapshots.change_id, decoded.change_id),
									)
									.limit(1);

								if (!content_row?.content) {
									return yield* Effect.fail(
										make_unavailable("read", decoded.change_id),
									);
								}

								const content = new Uint8Array(content_row.content);
								const actual_identity = yield* SnapshotIdentity(
									crypto,
									content,
								).pipe(
									Effect.mapError(() =>
										make_unavailable("read", decoded.change_id),
									),
								);

								if (
									content.byteLength !== metadata_row.byte_count ||
									!identities_match(actual_identity, decoded.expected_identity)
								) {
									return yield* Effect.fail(
										make_unavailable("read", decoded.change_id),
									);
								}

								return content;
							}),
						),
					),
				)
				.pipe(Effect.mapError((error) => conceal_error(error, "read", change_id)));
		};

		const Exists = (
			input: WorkspaceSnapshotExistsInput,
		): Effect.Effect<boolean, WorkspaceSnapshotStoreError> => {
			const change_id = change_id_from_unknown(input);

			return Decode(ExistsInput, input, "exists")
				.pipe(
					Effect.flatMap((decoded) =>
						database.client.transaction((transaction) =>
							Effect.gen(function* () {
								const canonical = yield* EnsureCanonicalReplace(
									transaction,
									decoded.thread_id,
									decoded.change_id,
									"exists",
								);

								if (
									canonical.lifecycle !== "committed" ||
									canonical.projection === undefined
								) {
									return yield* Effect.fail(
										make_unavailable("exists", decoded.change_id),
									);
								}

								if (canonical.projection.rollback_state === "consumed") {
									return false;
								}

								const [row] = yield* transaction
									.select({
										state: WorkspaceChangeSnapshots.state,
										thread_id: WorkspaceChangeSnapshots.thread_id,
									})
									.from(WorkspaceChangeSnapshots)
									.where(
										eq(WorkspaceChangeSnapshots.change_id, decoded.change_id),
									)
									.limit(1);

								if (!row || row.state !== "available") {
									return false;
								}

								if (row.thread_id !== decoded.thread_id) {
									return yield* Effect.fail(
										make_unavailable("exists", decoded.change_id),
									);
								}

								return true;
							}),
						),
					),
				)
				.pipe(Effect.mapError((error) => conceal_error(error, "exists", change_id)));
		};

		return { Consume, Exists, Read, Stage };
	}),
);
