import { desc, eq, or } from "drizzle-orm";
import { Context, Data, Effect, Layer, Option, Schema } from "effect";

import {
	EventEnvelope,
	HostedGitPullRequestLookup,
	HostedGitSnapshot,
	HostedGitSnapshotQuery,
	HostedGitSnapshotQueryResult,
	Identifier,
	IsoDateTime,
	ProjectRef,
	type EventEnvelope as EventEnvelopeValue,
	type HostedGitSnapshot as HostedGitSnapshotValue,
	type HostedGitSnapshotQueryResult as HostedGitSnapshotQueryResultValue,
} from "@artisan/protocol";

import { Database } from "../persistence/database";
import { JournalNotifier } from "../persistence/journal-notifier";
import { RetrySqliteWrite } from "../persistence/sqlite-write-retry";
import {
	EventStreams,
	HostedGitSnapshotOperations,
	HostedGitSnapshots,
	JournalCommands,
	JournalEvents,
	ProjectHostedOrigins,
	Projects,
	ThreadErasureClaims,
	Threads,
	ThreadTombstones,
} from "../persistence/schema";
import { JournalStoreFailure } from "../persistence/journal-store";
import { RuntimeMetadata } from "../runtime/runtime-metadata";

const RequestFingerprint = Schema.String.check(Schema.isPattern(/^[a-f0-9]{64}$/u));

const SourceCommandMetadata = Schema.Struct({
	message_id: Identifier,
	sent_at: IsoDateTime,
});

const ProjectSnapshotObservation = Schema.Struct({
	lookup: HostedGitPullRequestLookup,
	observed_at: IsoDateTime,
	operation_id: Identifier,
	project_id: Identifier,
	request_fingerprint: RequestFingerprint,
	source_command: SourceCommandMetadata,
	thread_id: Identifier,
	workspace_id: Identifier,
});

const ReplayProjection = Schema.Struct({
	operation_id: Identifier,
	project_id: Identifier,
	request_fingerprint: RequestFingerprint,
	sent_at: IsoDateTime,
	thread_id: Identifier,
	workspace_id: Identifier,
});

/** Supplies one exact provider observation before persistence assigns its version and cursor. */
export type ProjectHostedGitSnapshot = typeof ProjectSnapshotObservation.Type;

/** Returns the durable snapshot event accepted or replayed by one refresh command. */
export interface HostedGitSnapshotAcceptance {
	readonly event: EventEnvelopeValue;
	readonly snapshot: HostedGitSnapshotValue;
	readonly status: "accepted" | "duplicate";
}

/** Reports immutable operation or source-command reuse with different intent. */
export class HostedGitSnapshotConflict extends Data.TaggedError("HostedGitSnapshotConflict")<{
	readonly reason: "operation_conflict" | "source_command_conflict";
}> {}

/** Reports missing, erased, or unattached state without leaking private project metadata. */
export class HostedGitSnapshotUnavailable extends Data.TaggedError("HostedGitSnapshotUnavailable")<{
	readonly reason: "erased" | "missing" | "project_missing" | "thread_not_attached";
}> {}

/** Conceals corrupt persisted hosted-Git state behind one typed invariant failure. */
export class HostedGitSnapshotInvariant extends Data.TaggedError("HostedGitSnapshotInvariant")<{
	readonly message: string;
}> {}

/** Represents failures surfaced by the durable hosted-Git snapshot repository. */
export type HostedGitSnapshotRepositoryError =
	| HostedGitSnapshotConflict
	| HostedGitSnapshotInvariant
	| HostedGitSnapshotUnavailable
	| JournalStoreFailure;

/** Owns durable exact-head provider projections and their source command bindings. */
export class HostedGitSnapshotRepository extends Context.Service<
	HostedGitSnapshotRepository,
	{
		readonly Project: (
			input: ProjectHostedGitSnapshot,
		) => Effect.Effect<HostedGitSnapshotAcceptance, HostedGitSnapshotRepositoryError>;
		readonly Query: (
			query: typeof HostedGitSnapshotQuery.Type,
		) => Effect.Effect<HostedGitSnapshotQueryResultValue, HostedGitSnapshotRepositoryError>;
		readonly Replay: (
			input: typeof ReplayProjection.Type,
		) => Effect.Effect<
			Option.Option<HostedGitSnapshotAcceptance>,
			HostedGitSnapshotRepositoryError
		>;
	}
>()("Artisan/HostedGitSnapshotRepository") {}

type OperationRow = typeof HostedGitSnapshotOperations.$inferSelect;
type ProjectRow = typeof Projects.$inferSelect;
type SnapshotRow = typeof HostedGitSnapshots.$inferSelect;

function invariant(message: string) {
	return new HostedGitSnapshotInvariant({ message });
}

function normalize_error(error: unknown): HostedGitSnapshotRepositoryError {
	if (
		error instanceof HostedGitSnapshotConflict ||
		error instanceof HostedGitSnapshotInvariant ||
		error instanceof HostedGitSnapshotUnavailable ||
		error instanceof JournalStoreFailure
	) {
		return error;
	}

	return new JournalStoreFailure({ cause: error });
}

function snapshot_event_key(operation_id: string) {
	return `hosted_git_snapshot:${operation_id}`;
}

function refresh_command_payload(input: {
	readonly operation_id: string;
	readonly project_id: string;
	readonly request_fingerprint: string;
	readonly workspace_id: string;
}) {
	return JSON.stringify({
		operation_id: input.operation_id,
		project_id: input.project_id,
		request_fingerprint: input.request_fingerprint,
		type: "hosted.git.snapshot.refresh",
		workspace_id: input.workspace_id,
	});
}

function source_commands_match(
	row: typeof JournalCommands.$inferSelect,
	input: ProjectHostedGitSnapshot,
) {
	return (
		row.message_id === input.source_command.message_id &&
		row.schema_version === 1 &&
		row.thread_id === input.thread_id &&
		row.run_id === null &&
		row.agent_id === null &&
		row.causation_id === null &&
		row.origin === "frontend" &&
		row.raw_origin_json === null &&
		row.sent_at === input.source_command.sent_at &&
		row.payload_type === "hosted.git.snapshot.refresh" &&
		row.payload_json === refresh_command_payload(input) &&
		row.status === "accepted"
	);
}

/** Supplies the SQLite-backed hosted review and CI snapshot repository. */
export const HostedGitSnapshotRepositoryLive = Layer.effect(
	HostedGitSnapshotRepository,
	Effect.gen(function* () {
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;
		const notifier = yield* JournalNotifier;

		const DecodeProjectRef = (json: string, message: string) =>
			Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(json).pipe(
				Effect.flatMap(
					Schema.decodeUnknownEffect(ProjectRef, { onExcessProperty: "error" }),
				),
				Effect.mapError(() => invariant(message)),
			);

		const DecodeLinkedProjects = (json: string) =>
			Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(json).pipe(
				Effect.flatMap(
					Schema.decodeUnknownEffect(Schema.Array(ProjectRef), {
						onExcessProperty: "error",
					}),
				),
				Effect.mapError(() => invariant("Stored thread project links are corrupt")),
			);

		const EnsureProjectThread = (
			transaction: typeof database.client,
			input: {
				readonly lookup_repository: (typeof HostedGitPullRequestLookup.Type)["repository"];
				readonly project_id: string;
				readonly thread_id: string;
				readonly workspace_id: string;
			},
		) =>
			Effect.gen(function* () {
				const [project] = yield* transaction
					.select()
					.from(Projects)
					.where(eq(Projects.project_id, input.project_id))
					.limit(1);

				if (!project || project.workspace_id !== input.workspace_id) {
					return yield* new HostedGitSnapshotUnavailable({
						reason: "project_missing",
					});
				}

				const origins = yield* transaction
					.select()
					.from(ProjectHostedOrigins)
					.where(eq(ProjectHostedOrigins.project_id, project.project_id));

				if (origins.length !== 1) {
					return yield* invariant("Stored project hosted origin is corrupt");
				}

				const origin = origins[0]!;

				if (
					origin.provider_id !== input.lookup_repository.provider_id ||
					origin.canonical_host !== input.lookup_repository.host ||
					origin.owner !== input.lookup_repository.owner ||
					origin.name !== input.lookup_repository.name
				) {
					return yield* new HostedGitSnapshotConflict({
						reason: "operation_conflict",
					});
				}

				const [thread] = yield* transaction
					.select()
					.from(Threads)
					.where(eq(Threads.thread_id, input.thread_id))
					.limit(1);
				const [claim] = yield* transaction
					.select({ thread_id: ThreadErasureClaims.thread_id })
					.from(ThreadErasureClaims)
					.where(eq(ThreadErasureClaims.thread_id, input.thread_id))
					.limit(1);
				const [tombstone] = yield* transaction
					.select({ thread_id: ThreadTombstones.thread_id })
					.from(ThreadTombstones)
					.where(eq(ThreadTombstones.thread_id, input.thread_id))
					.limit(1);

				if (!thread || claim || tombstone) {
					return yield* new HostedGitSnapshotUnavailable({ reason: "erased" });
				}

				const linked_projects = yield* DecodeLinkedProjects(thread.linked_projects_json);
				const primary_project =
					thread.primary_project_id === null
						? undefined
						: thread.primary_project_json === null
							? yield* invariant("Stored primary thread project is missing")
							: yield* DecodeProjectRef(
									thread.primary_project_json,
									"Stored primary thread project is corrupt",
								);

				if (
					primary_project !== undefined &&
					primary_project.project_id !== thread.primary_project_id
				) {
					return yield* invariant("Stored primary thread project identity is corrupt");
				}

				const attached = [
					...(primary_project === undefined ? [] : [primary_project]),
					...linked_projects,
				].find((candidate) => candidate.project_id === project.project_id);

				if (!attached) {
					return yield* new HostedGitSnapshotUnavailable({
						reason: "thread_not_attached",
					});
				}

				if (
					attached.display_name !== project.display_name ||
					attached.root_path !== project.canonical_root
				) {
					return yield* invariant("Stored thread project reference is corrupt");
				}

				return project;
			});

		const DecodeEventRow = (row: typeof JournalEvents.$inferSelect) =>
			Effect.gen(function* () {
				const payload = yield* Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
					row.payload_json,
				).pipe(
					Effect.mapError(() => invariant("Stored hosted-Git event payload is corrupt")),
				);

				return yield* Schema.decodeUnknownEffect(EventEnvelope, {
					onExcessProperty: "error",
				})({
					causation_id: row.causation_id,
					correlation_id: row.correlation_id,
					journal_sequence: row.sequence,
					kind: "event",
					message_id: row.event_id,
					origin: row.origin,
					payload,
					protocol_version: 1,
					schema_version: row.schema_version,
					sent_at: row.occurred_at,
					sequence: row.stream_sequence,
					stream_id: row.stream_id,
					thread_id: row.thread_id,
				}).pipe(Effect.mapError(() => invariant("Stored hosted-Git event is corrupt")));
			});

		const ReadEvent = (transaction: typeof database.client, operation_id: string) =>
			transaction
				.select()
				.from(JournalEvents)
				.where(eq(JournalEvents.idempotency_key, snapshot_event_key(operation_id)))
				.limit(1)
				.pipe(
					Effect.flatMap(([row]) =>
						row
							? DecodeEventRow(row)
							: Effect.fail(invariant("Hosted-Git operation event is missing")),
					),
				);

		const DecodeSnapshot = (project: ProjectRow, row: SnapshotRow) =>
			Effect.gen(function* () {
				const lookup = yield* Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
					row.lookup_json,
				).pipe(
					Effect.flatMap(
						Schema.decodeUnknownEffect(HostedGitPullRequestLookup, {
							onExcessProperty: "error",
						}),
					),
					Effect.mapError(() => invariant("Stored hosted-Git lookup is corrupt")),
				);

				return yield* Schema.decodeUnknownEffect(HostedGitSnapshot, {
					onExcessProperty: "error",
				})({
					journal_sequence: row.journal_sequence,
					lookup,
					observed_at: row.observed_at,
					project_id: row.project_id,
					version: row.version,
					workspace_freshness: "unverified",
					workspace_id: project.workspace_id,
				}).pipe(Effect.mapError(() => invariant("Stored hosted-Git snapshot is corrupt")));
			});

		const ReadSnapshotByProject = (transaction: typeof database.client, project: ProjectRow) =>
			transaction
				.select()
				.from(HostedGitSnapshots)
				.where(eq(HostedGitSnapshots.project_id, project.project_id))
				.limit(1)
				.pipe(
					Effect.flatMap(([row]) =>
						row
							? DecodeSnapshot(project, row).pipe(
									Effect.map((snapshot) => ({ row, snapshot })),
								)
							: Effect.succeed(undefined),
					),
				);

		const ValidateObservation = (input: ProjectHostedGitSnapshot) =>
			Schema.decodeUnknownEffect(ProjectSnapshotObservation, {
				onExcessProperty: "error",
			})(input).pipe(
				Effect.mapError(
					() => new HostedGitSnapshotConflict({ reason: "operation_conflict" }),
				),
			);

		const ValidateReplay = (
			transaction: typeof database.client,
			project: ProjectRow,
			operation: OperationRow,
			input: ProjectHostedGitSnapshot,
		) =>
			Effect.gen(function* () {
				const event = yield* ReadEvent(transaction, operation.operation_id);

				if (event.payload.type !== "hosted.git.snapshot.updated") {
					return yield* invariant("Hosted-Git operation event has the wrong payload");
				}

				const snapshot = event.payload.snapshot;
				const [command] = yield* transaction
					.select()
					.from(JournalCommands)
					.where(eq(JournalCommands.message_id, input.source_command.message_id))
					.limit(1);
				const current = yield* ReadSnapshotByProject(transaction, project);

				if (
					operation.operation_id !== input.operation_id ||
					operation.source_command_id !== input.source_command.message_id ||
					operation.request_fingerprint !== input.request_fingerprint ||
					operation.thread_id !== input.thread_id ||
					operation.project_id !== input.project_id ||
					operation.workspace_id !== input.workspace_id ||
					operation.snapshot_version !== snapshot.version ||
					operation.journal_sequence !== snapshot.journal_sequence ||
					operation.sent_at !== input.source_command.sent_at ||
					!command ||
					!source_commands_match(command, input) ||
					current === undefined ||
					current.snapshot.version < snapshot.version ||
					(current.snapshot.version === snapshot.version &&
						JSON.stringify(current.snapshot) !== JSON.stringify(snapshot)) ||
					JSON.stringify(snapshot.lookup) !== JSON.stringify(input.lookup) ||
					snapshot.observed_at !== input.observed_at ||
					snapshot.project_id !== input.project_id ||
					snapshot.workspace_id !== input.workspace_id ||
					snapshot.workspace_freshness !== "unverified" ||
					event.agent_id !== undefined ||
					event.causation_id !== input.source_command.message_id ||
					event.correlation_id !== input.operation_id ||
					event.journal_sequence !== snapshot.journal_sequence ||
					event.origin !== "backend" ||
					event.raw_origin !== undefined ||
					event.run_id !== undefined ||
					event.sent_at !== input.observed_at ||
					event.stream_id !== `thread:${input.thread_id}` ||
					event.thread_id !== input.thread_id
				) {
					return yield* new HostedGitSnapshotConflict({ reason: "operation_conflict" });
				}

				return { event, snapshot, status: "duplicate" as const };
			});

		const AppendEvent = (
			transaction: typeof database.client,
			input: ProjectHostedGitSnapshot,
			snapshot: HostedGitSnapshotValue,
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
				const payload = { snapshot, type: "hosted.git.snapshot.updated" } as const;

				if (stream) {
					yield* transaction
						.update(EventStreams)
						.set({ last_sequence: stream_sequence })
						.where(eq(EventStreams.stream_id, stream_id));
				} else {
					yield* transaction
						.insert(EventStreams)
						.values({ last_sequence: stream_sequence, stream_id });
				}

				const [row] = yield* transaction
					.insert(JournalEvents)
					.values({
						causation_id: input.source_command.message_id,
						correlation_id: input.operation_id,
						event_id,
						event_type: payload.type,
						idempotency_key: snapshot_event_key(input.operation_id),
						occurred_at: input.observed_at,
						origin: "backend",
						payload_json: JSON.stringify(payload),
						schema_version: 1,
						stream_id,
						stream_sequence,
						thread_id: input.thread_id,
					})
					.returning();

				if (!row) {
					return yield* invariant("Hosted-Git event was not persisted");
				}

				return row;
			});

		const Project = (input: ProjectHostedGitSnapshot) =>
			ValidateObservation(input).pipe(
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const result = yield* RetrySqliteWrite(
							database.client.transaction((transaction) =>
								Effect.gen(function* () {
									const project = yield* EnsureProjectThread(transaction, {
										lookup_repository: decoded.lookup.repository,
										project_id: decoded.project_id,
										thread_id: decoded.thread_id,
										workspace_id: decoded.workspace_id,
									});
									const existing = yield* transaction
										.select()
										.from(HostedGitSnapshotOperations)
										.where(
											or(
												eq(
													HostedGitSnapshotOperations.operation_id,
													decoded.operation_id,
												),
												eq(
													HostedGitSnapshotOperations.source_command_id,
													decoded.source_command.message_id,
												),
											),
										)
										.limit(2);

									if (existing.length > 1) {
										return yield* new HostedGitSnapshotConflict({
											reason: "source_command_conflict",
										});
									}

									if (existing[0]) {
										if (existing[0].operation_id !== decoded.operation_id) {
											return yield* new HostedGitSnapshotConflict({
												reason: "source_command_conflict",
											});
										}

										return yield* ValidateReplay(
											transaction,
											project,
											existing[0],
											decoded,
										);
									}

									const [command] = yield* transaction
										.select({ message_id: JournalCommands.message_id })
										.from(JournalCommands)
										.where(
											eq(
												JournalCommands.message_id,
												decoded.source_command.message_id,
											),
										)
										.limit(1);

									if (command) {
										return yield* new HostedGitSnapshotConflict({
											reason: "source_command_conflict",
										});
									}

									const previous = yield* ReadSnapshotByProject(
										transaction,
										project,
									);
									const version = (previous?.snapshot.version ?? 0) + 1;
									const provisional: HostedGitSnapshotValue = {
										journal_sequence: 0,
										lookup: decoded.lookup,
										observed_at: decoded.observed_at,
										project_id: decoded.project_id,
										version,
										workspace_freshness: "unverified",
										workspace_id: decoded.workspace_id,
									};
									const event_row = yield* AppendEvent(
										transaction,
										decoded,
										provisional,
									);
									const snapshot: HostedGitSnapshotValue = {
										...provisional,
										journal_sequence: event_row.sequence,
									};
									const payload = {
										snapshot,
										type: "hosted.git.snapshot.updated",
									} as const;

									yield* transaction
										.update(JournalEvents)
										.set({ payload_json: JSON.stringify(payload) })
										.where(eq(JournalEvents.sequence, event_row.sequence));

									const accepted_at = yield* metadata.Now;

									yield* transaction.insert(JournalCommands).values({
										accepted_at,
										message_id: decoded.source_command.message_id,
										origin: "frontend",
										payload_json: refresh_command_payload(decoded),
										payload_type: "hosted.git.snapshot.refresh",
										schema_version: 1,
										sent_at: decoded.source_command.sent_at,
										status: "accepted",
										thread_id: decoded.thread_id,
									});

									yield* transaction
										.insert(HostedGitSnapshots)
										.values({
											journal_sequence: snapshot.journal_sequence,
											lookup_json: JSON.stringify(snapshot.lookup),
											observed_at: snapshot.observed_at,
											project_id: snapshot.project_id,
											version: snapshot.version,
										})
										.onConflictDoUpdate({
											set: {
												journal_sequence: snapshot.journal_sequence,
												lookup_json: JSON.stringify(snapshot.lookup),
												observed_at: snapshot.observed_at,
												version: snapshot.version,
											},
											target: HostedGitSnapshots.project_id,
										});

									yield* transaction.insert(HostedGitSnapshotOperations).values({
										journal_sequence: snapshot.journal_sequence,
										operation_id: decoded.operation_id,
										project_id: decoded.project_id,
										request_fingerprint: decoded.request_fingerprint,
										sent_at: decoded.source_command.sent_at,
										snapshot_version: snapshot.version,
										source_command_id: decoded.source_command.message_id,
										thread_id: decoded.thread_id,
										workspace_id: decoded.workspace_id,
									});

									const event = yield* ReadEvent(
										transaction,
										decoded.operation_id,
									);

									return { event, snapshot, status: "accepted" as const };
								}),
							),
						).pipe(Effect.mapError(normalize_error));

						if (result.status === "accepted") {
							yield* notifier.Publish(result.event.journal_sequence);
						}

						return result;
					}),
				),
			);

		const CurrentJournalSequence = (transaction: typeof database.client) =>
			transaction
				.select({ sequence: JournalEvents.sequence })
				.from(JournalEvents)
				.orderBy(desc(JournalEvents.sequence))
				.limit(1)
				.pipe(Effect.map(([row]) => row?.sequence ?? 0));

		const Query = (query: typeof HostedGitSnapshotQuery.Type) =>
			Schema.decodeUnknownEffect(HostedGitSnapshotQuery, {
				onExcessProperty: "error",
			})(query).pipe(
				Effect.mapError(() => new HostedGitSnapshotUnavailable({ reason: "missing" })),
				Effect.flatMap((decoded) =>
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							const [project] = yield* transaction
								.select()
								.from(Projects)
								.where(eq(Projects.workspace_id, decoded.workspace_id))
								.limit(1);
							const stored = project
								? yield* ReadSnapshotByProject(transaction, project)
								: undefined;
							const journal_sequence = yield* CurrentJournalSequence(transaction);
							const result = stored
								? { journal_sequence, snapshot: stored.snapshot }
								: { journal_sequence };

							return yield* Schema.decodeUnknownEffect(HostedGitSnapshotQueryResult, {
								onExcessProperty: "error",
							})(result).pipe(
								Effect.mapError(() =>
									invariant("Hosted-Git query result is corrupt"),
								),
							);
						}),
					),
				),
				Effect.mapError(normalize_error),
			);

		const Replay = (input: typeof ReplayProjection.Type) =>
			Schema.decodeUnknownEffect(ReplayProjection, { onExcessProperty: "error" })(input).pipe(
				Effect.mapError(
					() => new HostedGitSnapshotConflict({ reason: "operation_conflict" }),
				),
				Effect.flatMap((decoded) =>
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							const [operation] = yield* transaction
								.select()
								.from(HostedGitSnapshotOperations)
								.where(
									eq(
										HostedGitSnapshotOperations.operation_id,
										decoded.operation_id,
									),
								)
								.limit(1);

							if (!operation) {
								const [orphaned_event] = yield* transaction
									.select({ sequence: JournalEvents.sequence })
									.from(JournalEvents)
									.where(
										eq(
											JournalEvents.idempotency_key,
											snapshot_event_key(decoded.operation_id),
										),
									)
									.limit(1);

								if (orphaned_event) {
									return yield* invariant(
										"Hosted-Git event has no projection operation",
									);
								}

								return Option.none<HostedGitSnapshotAcceptance>();
							}

							const event = yield* ReadEvent(transaction, operation.operation_id);

							if (event.payload.type !== "hosted.git.snapshot.updated") {
								return yield* invariant(
									"Hosted-Git operation event has the wrong payload",
								);
							}

							const snapshot = event.payload.snapshot;
							const project = yield* EnsureProjectThread(transaction, {
								lookup_repository: snapshot.lookup.repository,
								project_id: decoded.project_id,
								thread_id: decoded.thread_id,
								workspace_id: decoded.workspace_id,
							});
							const current = yield* ReadSnapshotByProject(transaction, project);
							const [command] = yield* transaction
								.select()
								.from(JournalCommands)
								.where(eq(JournalCommands.message_id, operation.source_command_id))
								.limit(1);

							if (
								operation.source_command_id !== decoded.operation_id ||
								operation.request_fingerprint !== decoded.request_fingerprint ||
								operation.thread_id !== decoded.thread_id ||
								operation.project_id !== decoded.project_id ||
								operation.workspace_id !== decoded.workspace_id ||
								operation.snapshot_version !== snapshot.version ||
								operation.journal_sequence !== snapshot.journal_sequence ||
								operation.sent_at !== decoded.sent_at ||
								!command ||
								command.message_id !== operation.source_command_id ||
								command.schema_version !== 1 ||
								command.agent_id !== null ||
								command.causation_id !== null ||
								command.run_id !== null ||
								command.raw_origin_json !== null ||
								command.origin !== "frontend" ||
								command.payload_type !== "hosted.git.snapshot.refresh" ||
								command.payload_json !== refresh_command_payload(decoded) ||
								command.thread_id !== decoded.thread_id ||
								command.sent_at !== decoded.sent_at ||
								command.status !== "accepted" ||
								current === undefined ||
								current.snapshot.version < snapshot.version ||
								(current.snapshot.version === snapshot.version &&
									JSON.stringify(current.snapshot) !==
										JSON.stringify(snapshot)) ||
								snapshot.project_id !== decoded.project_id ||
								snapshot.workspace_id !== decoded.workspace_id ||
								snapshot.workspace_freshness !== "unverified" ||
								event.causation_id !== decoded.operation_id ||
								event.correlation_id !== decoded.operation_id ||
								event.journal_sequence !== snapshot.journal_sequence ||
								event.origin !== "backend" ||
								event.sent_at !== snapshot.observed_at ||
								event.stream_id !== `thread:${decoded.thread_id}` ||
								event.thread_id !== decoded.thread_id
							) {
								return yield* new HostedGitSnapshotConflict({
									reason: "operation_conflict",
								});
							}

							return Option.some({ event, snapshot, status: "duplicate" as const });
						}),
					),
				),
				Effect.mapError(normalize_error),
			);

		return { Project, Query, Replay };
	}),
);
