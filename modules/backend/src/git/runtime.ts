import { and, asc, desc, eq } from "drizzle-orm";
import { Context, Crypto, Effect, Encoding, Schema } from "effect";

import {
	EventEnvelope,
	EventPayload,
	GitMutationFailure,
	GitMutationPaths,
	GitMutationProjection,
	GitWorkspaceProjection,
	GitWorkspaceUpdatedEvent,
	RawOrigin,
	type GitWorkspaceProjection as GitWorkspaceProjectionValue,
} from "@artisan/protocol";

import { Database } from "../persistence/database";
import {
	EventStreams,
	GitMutationOperations,
	GitWorkspaceProjections,
	JournalEvents,
} from "../persistence/tables";
import { IsThreadLive } from "../persistence/thread-liveness";
import { RuntimeMetadata } from "../runtime/metadata";
import { RecordThreadActivity } from "../threads/internal/thread-activity";
import {
	GitRepositoryConflict,
	GitRepositoryInvalid,
	GitRepositoryInvariantError,
	GitRepositoryNotFound,
	GitRepositoryPersistenceFailure,
	GitWorkspaceObservation,
	StoredGitMutationRow,
	StoredGitWorkspaceRow,
	StoredJournalEventRow,
	type DecodedMutationRow,
	type GitRepositoryError,
	type GitWorkspaceRecordInput,
	type MutationIdentity,
} from "./contracts";

function normalize_error(error: unknown): GitRepositoryError {
	if (
		error instanceof GitRepositoryConflict ||
		error instanceof GitRepositoryInvalid ||
		error instanceof GitRepositoryInvariantError ||
		error instanceof GitRepositoryNotFound ||
		error instanceof GitRepositoryPersistenceFailure
	) {
		return error;
	}

	return new GitRepositoryPersistenceFailure({ cause: error });
}

function invariant(message: string) {
	return new GitRepositoryInvariantError({ message });
}

function canonical_paths(paths: typeof GitMutationPaths.Type) {
	return Schema.decodeUnknownSync(GitMutationPaths)(
		[...paths].toSorted((left, right) => (left < right ? -1 : left > right ? 1 : 0)),
	);
}

function optional_equal(left: string | undefined, right: string | undefined) {
	return left === right;
}

function raw_origins_equal(
	left: typeof RawOrigin.Type | undefined,
	right: typeof RawOrigin.Type | undefined,
) {
	return (
		(left === undefined && right === undefined) ||
		(left !== undefined &&
			right !== undefined &&
			left.provider === right.provider &&
			left.reference === right.reference)
	);
}

function traces_equal(
	left: {
		readonly agent_id?: string;
		readonly raw_origin?: typeof RawOrigin.Type;
		readonly run_id?: string;
		readonly thread_id: string;
	},
	right: {
		readonly agent_id?: string;
		readonly raw_origin?: typeof RawOrigin.Type;
		readonly run_id?: string;
		readonly thread_id: string;
	},
) {
	return (
		optional_equal(left.agent_id, right.agent_id) &&
		raw_origins_equal(left.raw_origin, right.raw_origin) &&
		optional_equal(left.run_id, right.run_id) &&
		left.thread_id === right.thread_id
	);
}

const Decode = <A>(schema: Schema.Codec<A, A>, input: unknown, operation: string) =>
	Schema.decodeUnknownEffect(schema, { onExcessProperty: "error" })(input).pipe(
		Effect.mapError(() => new GitRepositoryInvalid({ operation })),
	);

const DecodeJson = <A>(schema: Schema.Codec<A, A>, json: string, context: string) =>
	Schema.decodeUnknownEffect(Schema.fromJsonString(schema), {
		onExcessProperty: "error",
	})(json).pipe(Effect.mapError(() => invariant(`${context} contains invalid JSON`)));

export const MakeGitRuntime = Effect.gen(function* () {
	const crypto = yield* Crypto.Crypto;
	const database = yield* Database;
	const metadata = yield* RuntimeMetadata;
	const ComputeFingerprint = (identity: MutationIdentity) =>
		Effect.gen(function* () {
			const canonical = {
				agent_id: identity.agent_id ?? null,
				approval_id: identity.approval_id,
				expected_snapshot_id: identity.expected_snapshot_id,
				expected_workspace_version: identity.expected_workspace_version,
				kind: identity.kind,
				mutation_id: identity.mutation_id,
				paths: canonical_paths(identity.paths),
				raw_origin: identity.raw_origin ?? null,
				request_message_id: identity.request_message_id,
				requested_at: identity.requested_at,
				run_id: identity.run_id ?? null,
				thread_id: identity.thread_id,
				workspace_id: identity.workspace_id,
			};
			const digest = yield* crypto.digest(
				"SHA-256",
				new TextEncoder().encode(JSON.stringify(canonical)),
			);

			return Encoding.encodeHex(digest);
		}).pipe(Effect.mapError((cause) => new GitRepositoryPersistenceFailure({ cause })));

	const DecodeOptionalJson = <A>(
		schema: Schema.Codec<A, A>,
		json: string | null,
		context: string,
	) =>
		json === null
			? Effect.succeed(undefined)
			: DecodeJson(schema, json, context).pipe(
					Effect.mapError(() => invariant(`${context} does not match its schema`)),
				);

	const DecodeWorkspaceRow = (input: unknown) =>
		Schema.decodeUnknownEffect(StoredGitWorkspaceRow, {
			onExcessProperty: "error",
		})(input).pipe(
			Effect.mapError(() => invariant("Stored Git workspace row is invalid")),
			Effect.flatMap((row) =>
				DecodeJson(
					GitWorkspaceProjection,
					row.projection_json,
					"Stored Git workspace projection",
				).pipe(
					Effect.mapError(() =>
						invariant("Stored Git workspace projection does not match its schema"),
					),
					Effect.flatMap((projection) =>
						projection.workspace_id !== row.workspace_id ||
						projection.snapshot_id !== row.snapshot_id ||
						projection.version !== row.version ||
						projection.journal_sequence !== row.journal_sequence ||
						projection.observed_at !== row.observed_at ||
						row.journal_sequence <= 0
							? Effect.fail(invariant("Stored Git workspace aliases disagree"))
							: Effect.succeed(projection),
					),
				),
			),
		);

	const MakeWorkspaceProjection = (
		observation: GitWorkspaceObservation,
		version: number,
		journal_sequence: number,
	) =>
		Schema.decodeUnknownEffect(GitWorkspaceProjection, {
			onExcessProperty: "error",
		})({ ...observation, journal_sequence, version }).pipe(
			Effect.mapError(() => invariant("Git workspace observation is inconsistent")),
		);

	const MakeMutationProjection = (
		row: typeof StoredGitMutationRow.Type,
		paths: typeof GitMutationPaths.Type,
		raw_origin: typeof RawOrigin.Type | undefined,
		failure: typeof GitMutationFailure.Type | undefined,
		journal_sequence: number,
	) =>
		Schema.decodeUnknownEffect(GitMutationProjection, {
			onExcessProperty: "error",
		})({
			...(row.agent_id === null ? {} : { agent_id: row.agent_id }),
			approval_id: row.approval_id,
			...(row.completed_at === null ? {} : { completed_at: row.completed_at }),
			...(row.decision_at === null ? {} : { decision_at: row.decision_at }),
			...(row.decision_message_id === null
				? {}
				: { decision_message_id: row.decision_message_id }),
			...(row.dispatched_at === null ? {} : { dispatched_at: row.dispatched_at }),
			expected_snapshot_id: row.expected_snapshot_id,
			expected_workspace_version: row.expected_workspace_version,
			...(failure === undefined ? {} : { failure }),
			journal_sequence,
			kind: row.kind,
			lifecycle: row.lifecycle,
			mutation_id: row.mutation_id,
			paths,
			...(raw_origin === undefined ? {} : { raw_origin }),
			requested_at: row.requested_at,
			...(row.result_snapshot_id === null
				? {}
				: { result_snapshot_id: row.result_snapshot_id }),
			...(row.result_workspace_version === null
				? {}
				: { result_workspace_version: row.result_workspace_version }),
			...(row.run_id === null ? {} : { run_id: row.run_id }),
			source_message_id: row.source_message_id,
			thread_id: row.thread_id,
			updated_at: row.updated_at,
			workspace_id: row.workspace_id,
		}).pipe(
			Effect.mapError((cause) =>
				invariant(`Stored Git mutation ${row.mutation_id} is invalid: ${String(cause)}`),
			),
		);

	const DecodeMutationRow = (
		input: unknown,
		allow_provisional_journal_sequence = false,
	): Effect.Effect<
		DecodedMutationRow,
		GitRepositoryInvariantError | GitRepositoryPersistenceFailure
	> =>
		Effect.gen(function* () {
			const row = yield* Schema.decodeUnknownEffect(StoredGitMutationRow, {
				onExcessProperty: "error",
			})(input).pipe(Effect.mapError(() => invariant("Stored Git mutation row is invalid")));
			const paths = yield* DecodeJson(
				GitMutationPaths,
				row.paths_json,
				`Stored Git mutation ${row.mutation_id} paths`,
			).pipe(
				Effect.mapError(() =>
					invariant(`Stored Git mutation ${row.mutation_id} paths are invalid`),
				),
			);

			if (JSON.stringify(paths) !== JSON.stringify(canonical_paths(paths))) {
				return yield* Effect.fail(
					invariant(`Stored Git mutation ${row.mutation_id} paths are not canonical`),
				);
			}

			const raw_origin = yield* DecodeOptionalJson(
				RawOrigin,
				row.raw_origin_json,
				`Stored Git mutation ${row.mutation_id} raw origin`,
			);
			const failure = yield* DecodeOptionalJson(
				GitMutationFailure,
				row.failure_code,
				`Stored Git mutation ${row.mutation_id} failure`,
			);

			if (
				row.journal_sequence === null ||
				(!allow_provisional_journal_sequence && row.journal_sequence <= 0)
			) {
				return yield* Effect.fail(
					invariant(`Stored Git mutation ${row.mutation_id} has no journal event`),
				);
			}

			const terminal = ["denied", "succeeded", "failed", "ambiguous"].includes(row.lifecycle);
			const resolved = row.lifecycle !== "awaiting_approval";
			const dispatched = ["dispatching", "succeeded", "failed", "ambiguous"].includes(
				row.lifecycle,
			);
			const failed = row.lifecycle === "failed" || row.lifecycle === "ambiguous";
			const succeeded = row.lifecycle === "succeeded";
			const has_partial_dispatch_lease =
				(row.dispatch_owner_id === null) !== (row.dispatch_lease_expires_at === null);

			if (
				resolved !== (row.decision_message_id !== null && row.decision_at !== null) ||
				dispatched !== (row.dispatched_at !== null) ||
				has_partial_dispatch_lease ||
				terminal !== (row.completed_at !== null) ||
				failed !== (failure !== undefined) ||
				succeeded !==
					(row.result_snapshot_id !== null && row.result_workspace_version !== null) ||
				(!succeeded &&
					(row.result_snapshot_id !== null || row.result_workspace_version !== null))
			) {
				return yield* Effect.fail(
					invariant(`Stored Git mutation ${row.mutation_id} lifecycle aliases disagree`),
				);
			}

			const identity: MutationIdentity = {
				...(row.agent_id === null ? {} : { agent_id: row.agent_id }),
				approval_id: row.approval_id,
				expected_snapshot_id: row.expected_snapshot_id,
				expected_workspace_version: row.expected_workspace_version,
				kind: row.kind,
				mutation_id: row.mutation_id,
				paths,
				...(raw_origin === undefined ? {} : { raw_origin }),
				request_message_id: row.source_message_id,
				requested_at: row.requested_at,
				...(row.run_id === null ? {} : { run_id: row.run_id }),
				thread_id: row.thread_id,
				workspace_id: row.workspace_id,
			};
			const request_fingerprint = yield* ComputeFingerprint(identity);

			if (request_fingerprint !== row.request_fingerprint) {
				return yield* Effect.fail(
					invariant(`Stored Git mutation ${row.mutation_id} fingerprint is invalid`),
				);
			}

			const projection = yield* MakeMutationProjection(
				row,
				paths,
				raw_origin,
				failure,
				row.journal_sequence,
			);

			return {
				identity,
				projection,
				request_fingerprint,
				...(row.result_snapshot_id === null
					? {}
					: { result_snapshot_id: row.result_snapshot_id }),
				row,
			};
		});

	const DecodeEventRow = (input: unknown) =>
		Effect.gen(function* () {
			const row = yield* Schema.decodeUnknownEffect(StoredJournalEventRow, {
				onExcessProperty: "error",
			})(input).pipe(Effect.mapError(() => invariant("Stored Git event row is invalid")));
			const payload = yield* DecodeJson(
				EventPayload,
				row.payload_json,
				`Stored event ${row.event_id}`,
			).pipe(
				Effect.mapError(() => invariant(`Stored event ${row.event_id} payload is invalid`)),
			);
			const raw_origin = yield* DecodeOptionalJson(
				RawOrigin,
				row.raw_origin_json,
				`Stored event ${row.event_id} raw origin`,
			);

			if (payload.type !== row.event_type) {
				return yield* Effect.fail(invariant(`Stored event ${row.event_id} type disagrees`));
			}

			return yield* Schema.decodeUnknownEffect(EventEnvelope, {
				onExcessProperty: "error",
			})({
				...(row.agent_id === null ? {} : { agent_id: row.agent_id }),
				causation_id: row.causation_id,
				correlation_id: row.correlation_id,
				journal_sequence: row.journal_sequence,
				kind: "event",
				message_id: row.event_id,
				origin: "backend",
				payload,
				protocol_version: 1,
				...(raw_origin === undefined ? {} : { raw_origin }),
				...(row.run_id === null ? {} : { run_id: row.run_id }),
				schema_version: 1,
				sequence: row.stream_sequence,
				sent_at: row.occurred_at,
				stream_id: row.stream_id,
				thread_id: row.thread_id,
			}).pipe(
				Effect.mapError(() =>
					invariant(`Stored event ${row.event_id} envelope is invalid`),
				),
			);
		});

	const event_columns = {
		agent_id: JournalEvents.agent_id,
		causation_id: JournalEvents.causation_id,
		correlation_id: JournalEvents.correlation_id,
		event_id: JournalEvents.event_id,
		event_type: JournalEvents.event_type,
		journal_sequence: JournalEvents.sequence,
		occurred_at: JournalEvents.occurred_at,
		origin: JournalEvents.origin,
		payload_json: JournalEvents.payload_json,
		raw_origin_json: JournalEvents.raw_origin_json,
		run_id: JournalEvents.run_id,
		schema_version: JournalEvents.schema_version,
		stream_id: JournalEvents.stream_id,
		stream_sequence: JournalEvents.stream_sequence,
		thread_id: JournalEvents.thread_id,
	};

	const ReadEventBySequence = (transaction: typeof database.client, sequence: number) =>
		transaction
			.select(event_columns)
			.from(JournalEvents)
			.where(eq(JournalEvents.sequence, sequence))
			.limit(1)
			.pipe(
				Effect.flatMap(([row]) =>
					row === undefined
						? Effect.fail(invariant(`Git event ${sequence} is missing`))
						: DecodeEventRow(row),
				),
			);

	const ReadFirstEvent = (
		transaction: typeof database.client,
		correlation_id: string,
		event_type: "git.mutation.updated" | "git.workspace.updated",
	) =>
		transaction
			.select(event_columns)
			.from(JournalEvents)
			.where(
				and(
					eq(JournalEvents.correlation_id, correlation_id),
					eq(JournalEvents.event_type, event_type),
				),
			)
			.orderBy(asc(JournalEvents.sequence))
			.limit(1)
			.pipe(
				Effect.flatMap(([row]) =>
					row === undefined
						? Effect.fail(
								invariant(`Correlated Git event ${correlation_id} is missing`),
							)
						: DecodeEventRow(row),
				),
			);

	const ReadLastEvent = (
		transaction: typeof database.client,
		correlation_id: string,
		event_type: "git.mutation.updated" | "git.workspace.updated",
	) =>
		transaction
			.select(event_columns)
			.from(JournalEvents)
			.where(
				and(
					eq(JournalEvents.correlation_id, correlation_id),
					eq(JournalEvents.event_type, event_type),
				),
			)
			.orderBy(desc(JournalEvents.sequence))
			.limit(1)
			.pipe(
				Effect.flatMap(([row]) =>
					row === undefined
						? Effect.fail(
								invariant(`Correlated Git event ${correlation_id} is missing`),
							)
						: DecodeEventRow(row),
				),
			);

	const AppendEvent = (
		transaction: typeof database.client,
		input: {
			readonly agent_id?: string;
			readonly causation_id: string;
			readonly correlation_id: string;
			readonly occurred_at: string;
			readonly payload_at: (
				journal_sequence: number,
			) => Effect.Effect<typeof EventPayload.Type, GitRepositoryInvariantError>;
			readonly raw_origin?: typeof RawOrigin.Type;
			readonly run_id?: string;
			readonly thread_id: string;
		},
	) =>
		Effect.gen(function* () {
			const stream_id = `thread:${input.thread_id}`;
			const [stream] = yield* transaction
				.select({ last_sequence: EventStreams.last_sequence })
				.from(EventStreams)
				.where(eq(EventStreams.stream_id, stream_id))
				.limit(1);
			const stream_sequence = (stream?.last_sequence ?? 0) + 1;
			const event_id = yield* metadata.MakeId("event");
			const provisional_payload = yield* input.payload_at(0);

			if (stream === undefined) {
				yield* transaction.insert(EventStreams).values({
					last_sequence: stream_sequence,
					stream_id,
				});
			} else {
				yield* transaction
					.update(EventStreams)
					.set({ last_sequence: stream_sequence })
					.where(eq(EventStreams.stream_id, stream_id));
			}

			const [inserted] = yield* transaction
				.insert(JournalEvents)
				.values({
					agent_id: input.agent_id ?? null,
					causation_id: input.causation_id,
					correlation_id: input.correlation_id,
					event_id,
					event_type: provisional_payload.type,
					occurred_at: input.occurred_at,
					origin: "backend",
					payload_json: JSON.stringify(provisional_payload),
					raw_origin_json:
						input.raw_origin === undefined ? null : JSON.stringify(input.raw_origin),
					run_id: input.run_id ?? null,
					schema_version: 1,
					stream_id,
					stream_sequence,
					thread_id: input.thread_id,
				})
				.returning({ journal_sequence: JournalEvents.sequence });

			if (inserted === undefined) {
				return yield* Effect.fail(invariant("Git event reservation was not persisted"));
			}

			const payload = yield* input.payload_at(inserted.journal_sequence);
			yield* transaction
				.update(JournalEvents)
				.set({ payload_json: JSON.stringify(payload) })
				.where(eq(JournalEvents.event_id, event_id));
			yield* RecordThreadActivity(transaction, input.thread_id, input.occurred_at, payload);

			return yield* Schema.decodeUnknownEffect(EventEnvelope, {
				onExcessProperty: "error",
			})({
				...(input.agent_id === undefined ? {} : { agent_id: input.agent_id }),
				causation_id: input.causation_id,
				correlation_id: input.correlation_id,
				journal_sequence: inserted.journal_sequence,
				kind: "event",
				message_id: event_id,
				origin: "backend",
				payload,
				protocol_version: 1,
				...(input.raw_origin === undefined ? {} : { raw_origin: input.raw_origin }),
				...(input.run_id === undefined ? {} : { run_id: input.run_id }),
				schema_version: 1,
				sequence: stream_sequence,
				sent_at: input.occurred_at,
				stream_id,
				thread_id: input.thread_id,
			}).pipe(Effect.mapError(() => invariant(`Git event ${event_id} is invalid`)));
		});

	const EnsureLiveThread = (transaction: typeof database.client, thread_id: string) =>
		IsThreadLive(transaction, thread_id).pipe(
			Effect.filterOrFail(
				(live) => live,
				() => new GitRepositoryConflict({ reason: "thread_unavailable" }),
			),
			Effect.asVoid,
		);

	const ReadWorkspaceTransaction = (transaction: typeof database.client, workspace_id: string) =>
		Effect.gen(function* () {
			const [row] = yield* transaction
				.select()
				.from(GitWorkspaceProjections)
				.where(eq(GitWorkspaceProjections.workspace_id, workspace_id))
				.limit(1);

			if (row === undefined) {
				return yield* Effect.fail(new GitRepositoryNotFound({ resource: "workspace" }));
			}

			return yield* DecodeWorkspaceRow(row);
		});

	const ReadMutationTransaction = (transaction: typeof database.client, mutation_id: string) =>
		Effect.gen(function* () {
			const [row] = yield* transaction
				.select()
				.from(GitMutationOperations)
				.where(eq(GitMutationOperations.mutation_id, mutation_id))
				.limit(1);

			if (row === undefined) {
				return yield* Effect.fail(new GitRepositoryNotFound({ resource: "mutation" }));
			}

			return yield* DecodeMutationRow(row);
		});

	const WriteWorkspaceProjection = (
		transaction: typeof database.client,
		projection: GitWorkspaceProjectionValue,
		updated_at: string,
		expected?: GitWorkspaceProjectionValue,
	) =>
		Effect.gen(function* () {
			const values = {
				journal_sequence: projection.journal_sequence,
				observed_at: projection.observed_at,
				projection_json: JSON.stringify(projection),
				snapshot_id: projection.snapshot_id,
				updated_at,
				version: projection.version,
				workspace_id: projection.workspace_id,
			};

			if (expected === undefined) {
				const [inserted] = yield* transaction
					.insert(GitWorkspaceProjections)
					.values(values)
					.returning();

				if (inserted === undefined) {
					return yield* Effect.fail(
						invariant("Git workspace projection was not inserted"),
					);
				}
			} else {
				const [updated] = yield* transaction
					.update(GitWorkspaceProjections)
					.set(values)
					.where(
						and(
							eq(GitWorkspaceProjections.workspace_id, expected.workspace_id),
							eq(GitWorkspaceProjections.snapshot_id, expected.snapshot_id),
							eq(GitWorkspaceProjections.version, expected.version),
						),
					)
					.returning();

				if (updated === undefined) {
					return yield* Effect.fail(
						new GitRepositoryConflict({ reason: "workspace_changed" }),
					);
				}
			}
		});

	const RecordWorkspaceTransaction = (
		transaction: typeof database.client,
		input: GitWorkspaceRecordInput,
	) =>
		Effect.gen(function* () {
			yield* EnsureLiveThread(transaction, input.thread_id);
			const [dispatching] = yield* transaction
				.select({ mutation_id: GitMutationOperations.mutation_id })
				.from(GitMutationOperations)
				.where(
					and(
						eq(GitMutationOperations.workspace_id, input.workspace.workspace_id),
						eq(GitMutationOperations.lifecycle, "dispatching"),
					),
				)
				.limit(1);

			if (dispatching !== undefined) {
				return yield* Effect.fail(new GitRepositoryConflict({ reason: "workspace_busy" }));
			}

			const [stored] = yield* transaction
				.select()
				.from(GitWorkspaceProjections)
				.where(eq(GitWorkspaceProjections.workspace_id, input.workspace.workspace_id))
				.limit(1);
			const current = stored === undefined ? undefined : yield* DecodeWorkspaceRow(stored);

			if (current?.snapshot_id === input.workspace.snapshot_id) {
				const candidate = yield* MakeWorkspaceProjection(
					{ ...input.workspace, observed_at: current.observed_at },
					current.version,
					current.journal_sequence,
				);

				if (JSON.stringify(candidate) !== JSON.stringify(current)) {
					return yield* Effect.fail(
						invariant(
							"A Git snapshot identifier was reused for different workspace state",
						),
					);
				}

				return {
					event: yield* ReadEventBySequence(transaction, current.journal_sequence),
					status: "duplicate" as const,
					workspace: current,
				};
			}

			const version = (current?.version ?? 0) + 1;
			const updated_at = yield* metadata.Now;
			const event = yield* AppendEvent(transaction, {
				...(input.agent_id === undefined ? {} : { agent_id: input.agent_id }),
				causation_id: input.causation_id,
				correlation_id: input.correlation_id,
				occurred_at: updated_at,
				payload_at: (journal_sequence) =>
					MakeWorkspaceProjection(input.workspace, version, journal_sequence).pipe(
						Effect.flatMap((workspace) =>
							Schema.decodeUnknownEffect(GitWorkspaceUpdatedEvent, {
								onExcessProperty: "error",
							})({
								cause: input.cause,
								type: "git.workspace.updated",
								workspace,
							}),
						),
						Effect.mapError(() => invariant("Git workspace event is invalid")),
					),
				...(input.raw_origin === undefined ? {} : { raw_origin: input.raw_origin }),
				...(input.run_id === undefined ? {} : { run_id: input.run_id }),
				thread_id: input.thread_id,
			});
			const workspace = yield* MakeWorkspaceProjection(
				input.workspace,
				version,
				event.journal_sequence,
			);

			yield* WriteWorkspaceProjection(transaction, workspace, updated_at, current);

			return { event, status: "accepted" as const, workspace };
		});

	return {
		AppendEvent,
		ComputeFingerprint,
		Decode,
		DecodeEventRow,
		DecodeMutationRow,
		DecodeOptionalJson,
		DecodeWorkspaceRow,
		EnsureLiveThread,
		MakeMutationProjection,
		MakeWorkspaceProjection,
		ReadEventBySequence,
		ReadFirstEvent,
		ReadLastEvent,
		ReadMutationTransaction,
		ReadWorkspaceTransaction,
		RecordWorkspaceTransaction,
		WriteWorkspaceProjection,
		canonical_paths,
		invariant,
		normalize_error,
		traces_equal,
	};
});

export class GitRuntime extends Context.Service<
	GitRuntime,
	Effect.Success<typeof MakeGitRuntime>
>()("Artisan/GitRuntime") {}
