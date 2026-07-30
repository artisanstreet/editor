import { Context, Crypto, Data, Effect, Encoding, Layer, Schema } from "effect";
import { and, eq, or, sql } from "drizzle-orm";

import {
	ContentIdentity,
	Identifier,
	workspace_text_maximum_bytes,
	type ContentIdentity as ContentIdentityValue,
} from "@artisan/protocol";

import { Database } from "../../persistence/database";
import { RetrySqliteWrite } from "../../persistence/sqlite-write-retry";
import {
	ThreadErasureClaims,
	Threads,
	ThreadTombstones,
	WorkspaceChangeOperations,
	WorkspaceChanges,
	WorkspaceMutationPayloads,
} from "../../persistence/tables";
import { RuntimeMetadata } from "../../runtime/metadata";

const MutationAction = Schema.Union([Schema.Literal("replace"), Schema.Literal("rollback")]);
const PayloadBytes = Schema.Uint8Array.check(
	Schema.makeFilter<Uint8Array>((bytes) =>
		bytes.byteLength <= workspace_text_maximum_bytes
			? undefined
			: `Expected at most ${workspace_text_maximum_bytes} bytes`,
	),
);
const StageInput = Schema.Struct({
	action: MutationAction,
	expected: PayloadBytes,
	expected_identity: ContentIdentity,
	message_id: Identifier,
	replacement: PayloadBytes,
	replacement_identity: ContentIdentity,
	thread_id: Identifier,
});
const ResumeInput = Schema.Struct({
	action: MutationAction,
	expected_identity: ContentIdentity,
	message_id: Identifier,
	replacement_identity: ContentIdentity,
	thread_id: Identifier,
});
const HasRecordInput = ResumeInput;
const ConsumeInput = ResumeInput;
const ContentHash = /^[0-9a-f]{64}$/;

/** Supplies exact transient bytes for one controlled replace or rollback operation. */
export type WorkspaceMutationPayloadStageInput = typeof StageInput.Type;
/** Selects an exact transient payload for restart recovery. */
export type WorkspaceMutationPayloadResumeInput = typeof ResumeInput.Type;
/** Selects the canonical operation whose private payload-record presence should be checked. */
export type WorkspaceMutationPayloadHasRecordInput = typeof HasRecordInput.Type;
/** Selects the exact settled payload whose private bytes should be consumed. */
export type WorkspaceMutationPayloadConsumeInput = typeof ConsumeInput.Type;
/** Represents one recovered exact expected and replacement byte pair. */
export type WorkspaceMutationPayload = {
	readonly expected: Uint8Array;
	readonly replacement: Uint8Array;
};

/** Reports malformed private mutation-payload input before it reaches SQLite. */
export class WorkspaceMutationPayloadStoreInvalid extends Data.TaggedError(
	"WorkspaceMutationPayloadStoreInvalid",
)<{
	readonly message_id?: string;
	readonly operation: "consume" | "has_record" | "resume" | "stage";
}> {}
/** Reports a message ID already bound to a different available byte pair. */
export class WorkspaceMutationPayloadStoreConflict extends Data.TaggedError(
	"WorkspaceMutationPayloadStoreConflict",
)<{ readonly message_id: string; readonly operation: "stage" }> {}
/** Reports a missing, consumed, corrupt, erased, or otherwise unavailable payload. */
export class WorkspaceMutationPayloadStoreUnavailable extends Data.TaggedError(
	"WorkspaceMutationPayloadStoreUnavailable",
)<{
	readonly message_id?: string;
	readonly operation: "consume" | "has_record" | "resume" | "stage";
}> {}
/** Represents failures returned by private workspace mutation-payload operations. */
export type WorkspaceMutationPayloadStoreError =
	| WorkspaceMutationPayloadStoreConflict
	| WorkspaceMutationPayloadStoreInvalid
	| WorkspaceMutationPayloadStoreUnavailable;

/** Owns transient exact filesystem mutation bytes outside all durable public records. */
export class WorkspaceMutationPayloadStore extends Context.Service<
	WorkspaceMutationPayloadStore,
	{
		readonly Consume: (
			input: WorkspaceMutationPayloadConsumeInput,
		) => Effect.Effect<void, WorkspaceMutationPayloadStoreError>;
		readonly HasRecord: (
			input: WorkspaceMutationPayloadHasRecordInput,
		) => Effect.Effect<boolean, WorkspaceMutationPayloadStoreError>;
		readonly Resume: (
			input: WorkspaceMutationPayloadResumeInput,
		) => Effect.Effect<WorkspaceMutationPayload, WorkspaceMutationPayloadStoreError>;
		readonly Stage: (
			input: WorkspaceMutationPayloadStageInput,
		) => Effect.Effect<
			{ readonly status: "existing" | "staged" },
			WorkspaceMutationPayloadStoreError
		>;
	}
>()("Artisan/WorkspaceMutationPayloadStore") {}

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

function make_unavailable(
	operation: WorkspaceMutationPayloadStoreUnavailable["operation"],
	message_id?: string,
) {
	return new WorkspaceMutationPayloadStoreUnavailable({
		...(message_id === undefined ? {} : { message_id }),
		operation,
	});
}

function message_id_from_unknown(input: unknown) {
	return typeof input === "object" &&
		input !== null &&
		"message_id" in input &&
		typeof input.message_id === "string"
		? input.message_id
		: undefined;
}

function Decode<A>(
	schema: Schema.Codec<A, A>,
	input: unknown,
	operation: WorkspaceMutationPayloadStoreInvalid["operation"],
) {
	const message_id = message_id_from_unknown(input);

	return Schema.decodeUnknownEffect(schema, { onExcessProperty: "error" })(input).pipe(
		Effect.mapError(
			() =>
				new WorkspaceMutationPayloadStoreInvalid({
					...(message_id === undefined ? {} : { message_id }),
					operation,
				}),
		),
	);
}

function DecodeIdentity(
	value: string | null,
	operation: WorkspaceMutationPayloadStoreUnavailable["operation"],
	message_id: string,
) {
	if (value === null) return Effect.fail(make_unavailable(operation, message_id));

	return Schema.decodeUnknownEffect(Schema.fromJsonString(ContentIdentity), {
		onExcessProperty: "error",
	})(value).pipe(Effect.mapError(() => make_unavailable(operation, message_id)));
}

function BytesIdentity(crypto: Crypto.Crypto, bytes: Uint8Array) {
	return crypto.digest("SHA-256", bytes).pipe(
		Effect.map((digest) => ({
			algorithm: "sha256" as const,
			byte_count: bytes.byteLength,
			content_hash: Encoding.encodeHex(digest),
		})),
	);
}

/** Builds the SQLite-backed transient private mutation payload store. */
export const WorkspaceMutationPayloadStoreLive = Layer.effect(
	WorkspaceMutationPayloadStore,
	Effect.gen(function* () {
		const crypto = yield* Crypto.Crypto;
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;

		const EnsureLiveThread = (
			transaction: typeof database.client,
			thread_id: string,
			operation: WorkspaceMutationPayloadStoreUnavailable["operation"],
			message_id: string,
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

				if (!thread || claim || tombstone)
					return yield* Effect.fail(make_unavailable(operation, message_id));
			});

		const EnsureCanonical = (
			transaction: typeof database.client,
			input: WorkspaceMutationPayloadResumeInput,
			operation: WorkspaceMutationPayloadStoreUnavailable["operation"],
		) =>
			Effect.gen(function* () {
				yield* EnsureLiveThread(transaction, input.thread_id, operation, input.message_id);

				const [row] = yield* transaction
					.select({
						action: WorkspaceChangeOperations.action,
						change_id: WorkspaceChangeOperations.change_id,
						expected_identity_json: WorkspaceChangeOperations.expected_identity_json,
						lifecycle: WorkspaceChangeOperations.lifecycle,
						message_id: WorkspaceChangeOperations.message_id,
						result_identity_json: WorkspaceChangeOperations.result_identity_json,
						thread_id: WorkspaceChangeOperations.thread_id,
					})
					.from(WorkspaceChangeOperations)
					.where(eq(WorkspaceChangeOperations.message_id, input.message_id))
					.limit(1);
				if (
					!row ||
					row.action !== input.action ||
					row.thread_id !== input.thread_id ||
					(row.lifecycle !== "claimed" &&
						row.lifecycle !== "applied" &&
						row.lifecycle !== "committed" &&
						!(operation === "consume" && row.lifecycle === "rejected"))
				)
					return yield* Effect.fail(make_unavailable(operation, input.message_id));

				const expected_identity = yield* DecodeIdentity(
					row.expected_identity_json,
					operation,
					input.message_id,
				);
				let replacement_identity: ContentIdentityValue;

				if (input.action === "replace") {
					replacement_identity = yield* DecodeIdentity(
						row.result_identity_json,
						operation,
						input.message_id,
					);
					if (row.lifecycle === "rejected") {
						const [projection] = yield* transaction
							.select({ change_id: WorkspaceChanges.change_id })
							.from(WorkspaceChanges)
							.where(
								or(
									eq(WorkspaceChanges.change_id, row.change_id),
									eq(WorkspaceChanges.source_command_id, row.message_id),
								),
							)
							.limit(1);

						if (projection)
							return yield* Effect.fail(
								make_unavailable(operation, input.message_id),
							);
					}
				} else {
					const [change] = yield* transaction
						.select({
							after_identity_json: WorkspaceChanges.after_identity_json,
							before_identity_json: WorkspaceChanges.before_identity_json,
							review_state: WorkspaceChanges.review_state,
							rollback_state: WorkspaceChanges.rollback_state,
							source_command_id: WorkspaceChanges.source_command_id,
							thread_id: WorkspaceChanges.thread_id,
						})
						.from(WorkspaceChanges)
						.where(eq(WorkspaceChanges.change_id, row.change_id))
						.limit(1);
					const is_available_rollback =
						(row.lifecycle === "claimed" || row.lifecycle === "applied") &&
						(change?.review_state === "needs_review" ||
							change?.review_state === "reviewed") &&
						change.rollback_state === "available";
					const is_consumed_rollback =
						row.lifecycle === "committed" &&
						change?.review_state === "rolled_back" &&
						change.rollback_state === "consumed";
					const is_rejected_rollback =
						row.lifecycle === "rejected" &&
						(change?.review_state === "needs_review" ||
							change?.review_state === "reviewed") &&
						change.rollback_state === "available";

					if (
						!change ||
						change.thread_id !== input.thread_id ||
						change.source_command_id === input.message_id ||
						(!is_available_rollback && !is_consumed_rollback && !is_rejected_rollback)
					)
						return yield* Effect.fail(make_unavailable(operation, input.message_id));
					const before_identity = yield* DecodeIdentity(
						change.before_identity_json,
						operation,
						input.message_id,
					);
					const after_identity = yield* DecodeIdentity(
						change.after_identity_json,
						operation,
						input.message_id,
					);
					const [replace] = yield* transaction
						.select({
							action: WorkspaceChangeOperations.action,
							change_id: WorkspaceChangeOperations.change_id,
							expected_identity_json:
								WorkspaceChangeOperations.expected_identity_json,
							lifecycle: WorkspaceChangeOperations.lifecycle,
							result_identity_json: WorkspaceChangeOperations.result_identity_json,
							thread_id: WorkspaceChangeOperations.thread_id,
						})
						.from(WorkspaceChangeOperations)
						.where(eq(WorkspaceChangeOperations.message_id, change.source_command_id))
						.limit(1);

					if (
						!replace ||
						replace.action !== "replace" ||
						replace.change_id !== row.change_id ||
						replace.thread_id !== input.thread_id ||
						replace.lifecycle !== "committed"
					) {
						return yield* Effect.fail(make_unavailable(operation, input.message_id));
					}

					const replace_before = yield* DecodeIdentity(
						replace.expected_identity_json,
						operation,
						input.message_id,
					);
					const replace_after = yield* DecodeIdentity(
						replace.result_identity_json,
						operation,
						input.message_id,
					);
					if (!identities_match(expected_identity, after_identity))
						return yield* Effect.fail(make_unavailable(operation, input.message_id));
					if (!identities_match(before_identity, replace_before))
						return yield* Effect.fail(make_unavailable(operation, input.message_id));
					if (!identities_match(after_identity, replace_after))
						return yield* Effect.fail(make_unavailable(operation, input.message_id));
					replacement_identity = before_identity;
				}

				if (
					!identities_match(expected_identity, input.expected_identity) ||
					!identities_match(replacement_identity, input.replacement_identity)
				)
					return yield* Effect.fail(make_unavailable(operation, input.message_id));

				return { lifecycle: row.lifecycle };
			});

		const ReadAvailable = (
			transaction: typeof database.client,
			input: WorkspaceMutationPayloadResumeInput,
			operation: "consume" | "resume" | "stage",
		) =>
			Effect.gen(function* () {
				const [row] = yield* transaction
					.select({
						expected: WorkspaceMutationPayloads.expected,
						expected_byte_count: WorkspaceMutationPayloads.expected_byte_count,
						expected_hash: WorkspaceMutationPayloads.expected_hash,
						expected_length: sql<
							number | null
						>`length(${WorkspaceMutationPayloads.expected})`,
						replacement: WorkspaceMutationPayloads.replacement,
						replacement_byte_count: WorkspaceMutationPayloads.replacement_byte_count,
						replacement_hash: WorkspaceMutationPayloads.replacement_hash,
						replacement_length: sql<
							number | null
						>`length(${WorkspaceMutationPayloads.replacement})`,
						state: WorkspaceMutationPayloads.state,
						thread_id: WorkspaceMutationPayloads.thread_id,
					})
					.from(WorkspaceMutationPayloads)
					.where(eq(WorkspaceMutationPayloads.message_id, input.message_id))
					.limit(1);
				if (
					!row ||
					row.state !== "available" ||
					row.thread_id !== input.thread_id ||
					!row.expected ||
					!row.replacement ||
					typeof row.expected_byte_count !== "number" ||
					typeof row.replacement_byte_count !== "number" ||
					typeof row.expected_length !== "number" ||
					typeof row.replacement_length !== "number" ||
					typeof row.expected_hash !== "string" ||
					typeof row.replacement_hash !== "string" ||
					row.expected_byte_count < 0 ||
					row.expected_byte_count > workspace_text_maximum_bytes ||
					row.replacement_byte_count < 0 ||
					row.replacement_byte_count > workspace_text_maximum_bytes ||
					row.expected_length !== row.expected_byte_count ||
					row.replacement_length !== row.replacement_byte_count ||
					!ContentHash.test(row.expected_hash) ||
					!ContentHash.test(row.replacement_hash)
				)
					return yield* Effect.fail(make_unavailable(operation, input.message_id));

				const expected = new Uint8Array(row.expected);
				const replacement = new Uint8Array(row.replacement);
				const actual_expected = yield* BytesIdentity(crypto, expected).pipe(
					Effect.mapError(() => make_unavailable(operation, input.message_id)),
				);
				const actual_replacement = yield* BytesIdentity(crypto, replacement).pipe(
					Effect.mapError(() => make_unavailable(operation, input.message_id)),
				);
				if (
					!identities_match(actual_expected, input.expected_identity) ||
					!identities_match(actual_replacement, input.replacement_identity) ||
					actual_expected.content_hash !== row.expected_hash ||
					actual_replacement.content_hash !== row.replacement_hash
				)
					return yield* Effect.fail(make_unavailable(operation, input.message_id));

				return { expected, replacement };
			});

		const Stage = (
			input: WorkspaceMutationPayloadStageInput,
		): Effect.Effect<
			{ readonly status: "existing" | "staged" },
			WorkspaceMutationPayloadStoreError
		> => {
			const message_id = message_id_from_unknown(input);
			return Effect.gen(function* () {
				const decoded = yield* Decode(StageInput, input, "stage");
				const actual_expected = yield* BytesIdentity(crypto, decoded.expected).pipe(
					Effect.mapError(() => make_unavailable("stage", decoded.message_id)),
				);
				const actual_replacement = yield* BytesIdentity(crypto, decoded.replacement).pipe(
					Effect.mapError(() => make_unavailable("stage", decoded.message_id)),
				);
				if (
					!identities_match(actual_expected, decoded.expected_identity) ||
					!identities_match(actual_replacement, decoded.replacement_identity)
				)
					return yield* Effect.fail(
						new WorkspaceMutationPayloadStoreInvalid({
							message_id: decoded.message_id,
							operation: "stage",
						}),
					);
				const now = yield* metadata.Now;

				return yield* RetrySqliteWrite(
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							const canonical = yield* EnsureCanonical(transaction, decoded, "stage");
							const [preexisting] = yield* transaction
								.select({
									message_id: WorkspaceMutationPayloads.message_id,
									state: WorkspaceMutationPayloads.state,
								})
								.from(WorkspaceMutationPayloads)
								.where(eq(WorkspaceMutationPayloads.message_id, decoded.message_id))
								.limit(1);
							if (
								canonical.lifecycle !== "claimed" &&
								canonical.lifecycle !== "applied"
							)
								return yield* Effect.fail(
									make_unavailable("stage", decoded.message_id),
								);
							if (canonical.lifecycle === "applied" && !preexisting)
								return yield* Effect.fail(
									make_unavailable("stage", decoded.message_id),
								);
							const inserted = preexisting
								? []
								: yield* transaction
										.insert(WorkspaceMutationPayloads)
										.values({
											expected: Buffer.from(decoded.expected),
											expected_byte_count: actual_expected.byte_count,
											expected_hash: actual_expected.content_hash,
											message_id: decoded.message_id,
											replacement: Buffer.from(decoded.replacement),
											replacement_byte_count: actual_replacement.byte_count,
											replacement_hash: actual_replacement.content_hash,
											created_at: now,
											state: "available",
											thread_id: decoded.thread_id,
											updated_at: now,
										})
										.onConflictDoNothing()
										.returning({
											message_id: WorkspaceMutationPayloads.message_id,
										});
							const stored = yield* ReadAvailable(transaction, decoded, "stage");
							if (
								!bytes_match(stored.expected, decoded.expected) ||
								!bytes_match(stored.replacement, decoded.replacement)
							)
								return yield* Effect.fail(
									new WorkspaceMutationPayloadStoreConflict({
										message_id: decoded.message_id,
										operation: "stage",
									}),
								);

							return {
								status:
									inserted.length === 1
										? ("staged" as const)
										: ("existing" as const),
							};
						}),
					),
				);
			}).pipe(
				Effect.mapError((error) =>
					error instanceof WorkspaceMutationPayloadStoreConflict ||
					error instanceof WorkspaceMutationPayloadStoreInvalid ||
					error instanceof WorkspaceMutationPayloadStoreUnavailable
						? error
						: make_unavailable("stage", message_id),
				),
			);
		};

		const Resume = (
			input: WorkspaceMutationPayloadResumeInput,
		): Effect.Effect<WorkspaceMutationPayload, WorkspaceMutationPayloadStoreError> => {
			const message_id = message_id_from_unknown(input);
			return Decode(ResumeInput, input, "resume").pipe(
				Effect.flatMap((decoded) =>
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							const canonical = yield* EnsureCanonical(
								transaction,
								decoded,
								"resume",
							);
							if (
								canonical.lifecycle === "committed" ||
								canonical.lifecycle === "rejected"
							)
								return yield* Effect.fail(
									make_unavailable("resume", decoded.message_id),
								);
							const payload = yield* ReadAvailable(transaction, decoded, "resume");
							return {
								expected: new Uint8Array(payload.expected),
								replacement: new Uint8Array(payload.replacement),
							};
						}),
					),
				),
				Effect.mapError((error) =>
					error instanceof WorkspaceMutationPayloadStoreInvalid ||
					error instanceof WorkspaceMutationPayloadStoreUnavailable
						? error
						: make_unavailable("resume", message_id),
				),
			);
		};

		const HasRecord = (
			input: WorkspaceMutationPayloadHasRecordInput,
		): Effect.Effect<boolean, WorkspaceMutationPayloadStoreError> => {
			const message_id = message_id_from_unknown(input);

			return Decode(HasRecordInput, input, "has_record").pipe(
				Effect.flatMap((decoded) =>
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							yield* EnsureCanonical(transaction, decoded, "has_record");

							const [stored] = yield* transaction
								.select({
									message_id: WorkspaceMutationPayloads.message_id,
									thread_id: WorkspaceMutationPayloads.thread_id,
								})
								.from(WorkspaceMutationPayloads)
								.where(eq(WorkspaceMutationPayloads.message_id, decoded.message_id))
								.limit(1);

							if (!stored) {
								return false;
							}

							if (stored.thread_id !== decoded.thread_id) {
								return yield* Effect.fail(
									make_unavailable("has_record", decoded.message_id),
								);
							}

							return true;
						}),
					),
				),
				Effect.mapError((error) =>
					error instanceof WorkspaceMutationPayloadStoreInvalid ||
					error instanceof WorkspaceMutationPayloadStoreUnavailable
						? error
						: make_unavailable("has_record", message_id),
				),
			);
		};

		const Consume = (
			input: WorkspaceMutationPayloadConsumeInput,
		): Effect.Effect<void, WorkspaceMutationPayloadStoreError> => {
			const message_id = message_id_from_unknown(input);
			return Decode(ConsumeInput, input, "consume")
				.pipe(
					Effect.flatMap((decoded) =>
						metadata.Now.pipe(
							Effect.flatMap((now) =>
								RetrySqliteWrite(
									database.client.transaction((transaction) =>
										Effect.gen(function* () {
											const canonical = yield* EnsureCanonical(
												transaction,
												decoded,
												"consume",
											);

											if (
												canonical.lifecycle !== "committed" &&
												canonical.lifecycle !== "rejected"
											)
												return yield* Effect.fail(
													make_unavailable("consume", decoded.message_id),
												);

											const [stored] = yield* transaction
												.select({
													expected: WorkspaceMutationPayloads.expected,
													expected_byte_count:
														WorkspaceMutationPayloads.expected_byte_count,
													expected_hash:
														WorkspaceMutationPayloads.expected_hash,
													replacement:
														WorkspaceMutationPayloads.replacement,
													replacement_byte_count:
														WorkspaceMutationPayloads.replacement_byte_count,
													replacement_hash:
														WorkspaceMutationPayloads.replacement_hash,
													state: WorkspaceMutationPayloads.state,
													thread_id: WorkspaceMutationPayloads.thread_id,
												})
												.from(WorkspaceMutationPayloads)
												.where(
													eq(
														WorkspaceMutationPayloads.message_id,
														decoded.message_id,
													),
												)
												.limit(1);

											if (!stored && canonical.lifecycle === "rejected") {
												return;
											}

											if (!stored || stored.thread_id !== decoded.thread_id)
												return yield* Effect.fail(
													make_unavailable("consume", decoded.message_id),
												);
											if (stored.state === "consumed") {
												if (
													stored.expected !== null ||
													stored.expected_byte_count !== null ||
													stored.expected_hash !== null ||
													stored.replacement !== null ||
													stored.replacement_byte_count !== null ||
													stored.replacement_hash !== null
												)
													return yield* Effect.fail(
														make_unavailable(
															"consume",
															decoded.message_id,
														),
													);

												return;
											}

											yield* ReadAvailable(transaction, decoded, "consume");

											const consumed = yield* transaction
												.update(WorkspaceMutationPayloads)
												.set({
													expected: null,
													expected_byte_count: null,
													expected_hash: null,
													replacement: null,
													replacement_byte_count: null,
													replacement_hash: null,
													state: "consumed",
													updated_at: now,
												})
												.where(
													and(
														eq(
															WorkspaceMutationPayloads.message_id,
															decoded.message_id,
														),
														eq(
															WorkspaceMutationPayloads.thread_id,
															decoded.thread_id,
														),
														eq(
															WorkspaceMutationPayloads.state,
															"available",
														),
													),
												)
												.returning({
													message_id:
														WorkspaceMutationPayloads.message_id,
												});

											if (consumed.length !== 1)
												return yield* Effect.fail(
													make_unavailable("consume", decoded.message_id),
												);
										}),
									),
								),
							),
						),
					),
				)
				.pipe(
					Effect.mapError((error) =>
						error instanceof WorkspaceMutationPayloadStoreInvalid ||
						error instanceof WorkspaceMutationPayloadStoreUnavailable
							? error
							: make_unavailable("consume", message_id),
					),
				);
		};

		return { Consume, HasRecord, Resume, Stage };
	}),
);
