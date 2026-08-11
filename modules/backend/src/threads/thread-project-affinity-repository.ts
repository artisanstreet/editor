import { and, asc, eq, or } from "drizzle-orm";
import { Context, Data, Effect, Layer, Schema } from "effect";

import {
	EventEnvelope,
	Identifier,
	IsoDateTime,
	JournalSequence,
	ProjectAffinityEvidenceKind,
	ProjectRef,
	StreamSequence,
	ThreadListItem,
	type CommandEnvelope,
	type EventEnvelope as Event,
	type EventPayload,
	type ProjectRef as Project,
	type ThreadProjectRehomeSuggestion,
} from "@artisan/protocol";

import { Database } from "../persistence/database";
import { JournalNotifier } from "../persistence/journal-notifier";
import {
	CommandIdConflict,
	JournalInvariantError,
	JournalStoreFailure,
} from "../persistence/journal-store";
import {
	EventStreams,
	JournalCommands,
	JournalEvents,
	Projects,
	ThreadErasureClaims,
	ThreadProjectAffinityEvidence,
	Threads,
	ThreadTombstones,
} from "../persistence/tables";
import { RuntimeMetadata } from "../runtime/metadata";
import { DecodeThreadProjection } from "./internal/thread-projection";
import {
	decide_project_affinity,
	type ProjectAffinityDecision,
	type ProjectAffinityEvidence,
} from "./project-affinity-policy";

type ThreadDatabase = Context.Service.Shape<typeof Database>;
type ThreadTransaction = ThreadDatabase["client"];
type ThreadRow = typeof Threads.$inferSelect;
type EvidenceRow = typeof ThreadProjectAffinityEvidence.$inferSelect;
type AffinityUpdatedPayload = Extract<
	EventPayload,
	{ readonly type: "thread.project_affinity.updated" }
>;
type AffinityCommandPayload = Extract<
	CommandEnvelope["payload"],
	{
		readonly type: "thread.project.assign" | "thread.project.unlock";
	}
>;

interface AffinityProjectionValues {
	readonly linked_projects: ReadonlyArray<Project>;
	readonly primary_project?: Project;
	readonly project_affinity_scores: ThreadListItem["project_affinity_scores"];
	readonly project_locked: boolean;
	readonly rehome_suggestion?: ThreadProjectRehomeSuggestion;
}

interface AffinityTransition {
	readonly change: AffinityUpdatedPayload["change"];
	readonly projection: ThreadListItem;
}

interface AppendEventInput {
	readonly agent_id?: string;
	readonly causation_id: string;
	readonly correlation_id: string;
	readonly occurred_at: string;
	readonly payload: EventPayload;
	readonly raw_origin?: CommandEnvelope["raw_origin"];
	readonly run_id?: string;
	readonly thread_id: string;
}

/** Supplies one durable, content-free signal for a candidate project's ownership. */
export const ThreadProjectAffinityEvidenceInput = Schema.Struct({
	basis_affinity_version: StreamSequence,
	evidence_id: Identifier,
	kind: ProjectAffinityEvidenceKind,
	observed_at: IsoDateTime,
	project: ProjectRef,
	source_event_id: Identifier,
	source_journal_sequence: JournalSequence,
	thread_id: Identifier,
});

export type ThreadProjectAffinityEvidenceInput = typeof ThreadProjectAffinityEvidenceInput.Type;

export class ThreadProjectAffinityNotFound extends Data.TaggedError(
	"ThreadProjectAffinityNotFound",
)<{
	readonly thread_id: string;
}> {}

export type ThreadProjectAffinityError =
	| CommandIdConflict
	| JournalInvariantError
	| JournalStoreFailure
	| ThreadProjectAffinityNotFound;

/** Returns the canonical result of one affinity mutation or observed evidence row. */
export interface ThreadProjectAffinityAcceptance {
	readonly event: Event;
	readonly status: "accepted" | "duplicate";
}

/** Owns atomic affinity commands and idempotent source-evidence observation. */
export class ThreadProjectAffinityRepository extends Context.Service<
	ThreadProjectAffinityRepository,
	{
		readonly Accept: (
			command: CommandEnvelope,
		) => Effect.Effect<ThreadProjectAffinityAcceptance, ThreadProjectAffinityError>;
		readonly ObserveEvidence: (
			input: ThreadProjectAffinityEvidenceInput,
		) => Effect.Effect<ThreadProjectAffinityAcceptance, ThreadProjectAffinityError>;
	}
>()("Artisan/ThreadProjectAffinityRepository") {}

function serialize_project(project: Project) {
	return JSON.stringify({
		display_name: project.display_name,
		project_id: project.project_id,
		root_path: project.root_path,
	});
}

function command_matches(command: CommandEnvelope, existing: typeof JournalCommands.$inferSelect) {
	return (
		existing.agent_id === (command.agent_id ?? null) &&
		existing.causation_id === (command.causation_id ?? null) &&
		existing.origin === command.origin &&
		existing.payload_json === JSON.stringify(command.payload) &&
		existing.raw_origin_json ===
			(command.raw_origin ? JSON.stringify(command.raw_origin) : null) &&
		existing.run_id === (command.run_id ?? null) &&
		existing.schema_version === command.schema_version &&
		existing.sent_at === command.sent_at &&
		existing.thread_id === command.thread_id
	);
}

function is_affinity_command_payload(
	payload: CommandEnvelope["payload"],
): payload is AffinityCommandPayload {
	return payload.type === "thread.project.assign" || payload.type === "thread.project.unlock";
}

function evidence_matches(input: ThreadProjectAffinityEvidenceInput, existing: EvidenceRow) {
	return (
		existing.kind === input.kind &&
		existing.observed_at === input.observed_at &&
		existing.project_id === input.project.project_id &&
		existing.project_json === serialize_project(input.project) &&
		existing.source_event_id === input.source_event_id &&
		existing.source_journal_sequence === input.source_journal_sequence &&
		existing.thread_id === input.thread_id
	);
}

function unique_projects(projects: ReadonlyArray<Project>, primary_project: Project | undefined) {
	const unique = new Map<string, Project>();

	for (const project of projects) {
		if (project.project_id !== primary_project?.project_id) {
			unique.set(project.project_id, project);
		}
	}

	return [...unique.values()].slice(0, 3);
}

function normalize_error(error: unknown): ThreadProjectAffinityError {
	if (
		error instanceof CommandIdConflict ||
		error instanceof JournalInvariantError ||
		error instanceof ThreadProjectAffinityNotFound
	) {
		return error;
	}

	return new JournalStoreFailure({ cause: error });
}

const DecodeJson = <A>(
	value: string,
	schema: Schema.ConstraintDecoder<A, never>,
	context: string,
) =>
	Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(value).pipe(
		Effect.mapError(
			() => new JournalInvariantError({ message: `${context} JSON is malformed` }),
		),
		Effect.flatMap(
			Schema.decodeUnknownEffect(schema, {
				onExcessProperty: "error",
			}),
		),
		Effect.mapError(
			() => new JournalInvariantError({ message: `${context} does not match its schema` }),
		),
	);

const DecodeEvidence = (rows: ReadonlyArray<EvidenceRow>) =>
	Effect.forEach(rows, (row) =>
		Effect.all({
			kind: Schema.decodeUnknownEffect(ProjectAffinityEvidenceKind)(row.kind).pipe(
				Effect.mapError(
					() =>
						new JournalInvariantError({
							message: `Affinity evidence ${row.evidence_id} has an invalid kind`,
						}),
				),
			),
			project: DecodeJson(
				row.project_json,
				ProjectRef,
				`Affinity evidence ${row.evidence_id} project`,
			),
		}).pipe(
			Effect.flatMap(({ kind, project }) => {
				if (project.project_id !== row.project_id) {
					return Effect.fail(
						new JournalInvariantError({
							message: `Affinity evidence ${row.evidence_id} project identity is inconsistent`,
						}),
					);
				}

				return Effect.succeed({
					kind,
					project,
					source_journal_sequence: row.source_journal_sequence,
				} satisfies ProjectAffinityEvidence);
			}),
		),
	);

const MakeProjection = (
	current: ThreadListItem,
	values: AffinityProjectionValues,
	occurred_at: string,
) =>
	Schema.decodeUnknownEffect(ThreadListItem, {
		onExcessProperty: "error",
	})({
		activity_version: current.activity_version,
		affinity_version: current.affinity_version + 1,
		...(current.archived_at === undefined ? {} : { archived_at: current.archived_at }),
		created_at: current.created_at,
		...(current.current_goal === undefined ? {} : { current_goal: current.current_goal }),
		last_activity_at: current.last_activity_at,
		...(current.reader_activity_at === undefined
			? {}
			: { reader_activity_at: current.reader_activity_at }),
		...(current.last_assistant_message === undefined
			? {}
			: { last_assistant_message: current.last_assistant_message }),
		linked_projects: [...values.linked_projects],
		live_status: current.live_status,
		metadata_version: current.metadata_version,
		pinned: current.pinned,
		...(values.primary_project === undefined
			? {}
			: { primary_project: values.primary_project }),
		project_affinity_scores: [...values.project_affinity_scores],
		project_locked: values.project_locked,
		...(current.rename_suggestion === undefined
			? {}
			: { rename_suggestion: current.rename_suggestion }),
		...(values.rehome_suggestion === undefined
			? {}
			: { rehome_suggestion: values.rehome_suggestion }),
		thread_id: current.thread_id,
		title: current.title,
		title_locked: current.title_locked,
		title_source: current.title_source,
		updated_at: current.updated_at < occurred_at ? occurred_at : current.updated_at,
	}).pipe(
		Effect.mapError(
			() =>
				new JournalInvariantError({
					message: `Thread ${current.thread_id} affinity update is invalid`,
				}),
		),
	);

const BuildAutomaticProjection = (
	current: ThreadListItem,
	evidence: ReadonlyArray<ProjectAffinityEvidence>,
	occurred_at: string,
): Effect.Effect<AffinityTransition, JournalInvariantError> => {
	const decision: ProjectAffinityDecision =
		evidence.length === 0
			? {
					linked_projects: current.linked_projects,
					scores: current.project_affinity_scores,
				}
			: decide_project_affinity(evidence);
	const primary_project = decision.primary_project ?? current.primary_project;
	const suggested_project = decision.rehome_suggestion?.project;
	const rehome_suggestion =
		suggested_project &&
		decision.rehome_suggestion !== undefined &&
		suggested_project.project_id !== primary_project?.project_id
			? {
					basis_affinity_version: current.affinity_version + 1,
					project: suggested_project,
					score: decision.rehome_suggestion.score,
				}
			: undefined;
	const primary_changed = primary_project?.project_id !== current.primary_project?.project_id;
	const linked_projects = unique_projects(
		[
			...decision.linked_projects,
			...(primary_changed && current.primary_project ? [current.primary_project] : []),
			...current.linked_projects,
		],
		primary_project,
	);
	const change: AffinityUpdatedPayload["change"] = primary_changed
		? "rehomed"
		: rehome_suggestion
			? "suggested"
			: "observed";

	return MakeProjection(
		current,
		{
			linked_projects,
			...(primary_project === undefined ? {} : { primary_project }),
			project_affinity_scores: decision.scores,
			project_locked: false,
			...(rehome_suggestion === undefined ? {} : { rehome_suggestion }),
		},
		occurred_at,
	).pipe(Effect.map((projection) => ({ change, projection })));
};

export const ThreadProjectAffinityRepositoryLive = Layer.effect(
	ThreadProjectAffinityRepository,
	Effect.gen(function* () {
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;
		const notifier = yield* JournalNotifier;

		const ReadThread = (transaction: ThreadTransaction, thread_id: string) =>
			Effect.gen(function* () {
				const [thread] = yield* transaction
					.select()
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
					return yield* new ThreadProjectAffinityNotFound({ thread_id });
				}

				return thread;
			});

		const ReadEvidence = (transaction: ThreadTransaction, thread_id: string) =>
			transaction
				.select()
				.from(ThreadProjectAffinityEvidence)
				.where(eq(ThreadProjectAffinityEvidence.thread_id, thread_id))
				.orderBy(
					asc(ThreadProjectAffinityEvidence.observed_at),
					asc(ThreadProjectAffinityEvidence.evidence_id),
				)
				.pipe(Effect.flatMap(DecodeEvidence));

		const WriteProjection = (
			transaction: ThreadTransaction,
			thread: ThreadRow,
			projection: ThreadListItem,
		) =>
			transaction
				.update(Threads)
				.set({
					affinity_version: projection.affinity_version,
					linked_projects_json: JSON.stringify(projection.linked_projects),
					primary_project_id: projection.primary_project?.project_id ?? null,
					primary_project_json: projection.primary_project
						? serialize_project(projection.primary_project)
						: null,
					project_affinity_scores_json: JSON.stringify(
						projection.project_affinity_scores,
					),
					project_locked: projection.project_locked,
					rehome_suggestion_json: projection.rehome_suggestion
						? JSON.stringify(projection.rehome_suggestion)
						: null,
					updated_at: projection.updated_at,
				})
				.where(eq(Threads.thread_id, thread.thread_id))
				.pipe(Effect.asVoid);

		const AppendEvent = (transaction: ThreadTransaction, input: AppendEventInput) =>
			Effect.gen(function* () {
				const stream_id = `thread:${input.thread_id}`;
				const [stream] = yield* transaction
					.select({ last_sequence: EventStreams.last_sequence })
					.from(EventStreams)
					.where(eq(EventStreams.stream_id, stream_id))
					.limit(1);

				if (!stream) {
					return yield* new JournalInvariantError({
						message: `Thread ${input.thread_id} has no event stream`,
					});
				}

				const sequence = stream.last_sequence + 1;
				const event_id = yield* metadata.MakeId("event");

				yield* transaction
					.update(EventStreams)
					.set({ last_sequence: sequence })
					.where(eq(EventStreams.stream_id, stream_id));

				const [inserted] = yield* transaction
					.insert(JournalEvents)
					.values({
						agent_id: input.agent_id ?? null,
						causation_id: input.causation_id,
						correlation_id: input.correlation_id,
						event_id,
						event_type: input.payload.type,
						occurred_at: input.occurred_at,
						origin: "backend",
						payload_json: JSON.stringify(input.payload),
						raw_origin_json: input.raw_origin ? JSON.stringify(input.raw_origin) : null,
						run_id: input.run_id ?? null,
						schema_version: 1,
						stream_id,
						stream_sequence: sequence,
						thread_id: input.thread_id,
					})
					.returning({ journal_sequence: JournalEvents.sequence });
				if (inserted === undefined)
					return yield* new JournalInvariantError({
						message: `Affinity event ${event_id} returned no inserted row`,
					});

				return {
					...(input.agent_id ? { agent_id: input.agent_id } : {}),
					causation_id: input.causation_id,
					correlation_id: input.correlation_id,
					journal_sequence: inserted.journal_sequence,
					kind: "event" as const,
					message_id: event_id,
					origin: "backend" as const,
					payload: input.payload,
					protocol_version: 1 as const,
					...(input.raw_origin ? { raw_origin: input.raw_origin } : {}),
					...(input.run_id ? { run_id: input.run_id } : {}),
					schema_version: 1 as const,
					sent_at: input.occurred_at,
					sequence,
					stream_id,
					thread_id: input.thread_id,
				} satisfies Event;
			});

		const ReadEvent = (correlation_id: string) =>
			database.client
				.select()
				.from(JournalEvents)
				.where(
					and(
						eq(JournalEvents.correlation_id, correlation_id),
						or(
							eq(JournalEvents.event_type, "thread.project_affinity.updated"),
							eq(JournalEvents.event_type, "thread.project_affinity.ignored"),
						),
					),
				)
				.orderBy(asc(JournalEvents.sequence))
				.limit(1)
				.pipe(
					Effect.flatMap(([row]) => {
						if (!row) {
							return Effect.fail(
								new JournalInvariantError({
									message: `Affinity acceptance ${correlation_id} has no event`,
								}),
							);
						}

						return Effect.all({
							payload: DecodeJson(
								row.payload_json,
								EventEnvelope.fields.payload,
								`Affinity event ${row.event_id} payload`,
							),
							raw_origin:
								row.raw_origin_json === null
									? Effect.succeed(undefined)
									: DecodeJson(
											row.raw_origin_json,
											EventEnvelope.fields.raw_origin,
											`Affinity event ${row.event_id} raw origin`,
										),
						}).pipe(
							Effect.flatMap(({ payload, raw_origin }) =>
								Schema.decodeUnknownEffect(EventEnvelope, {
									onExcessProperty: "error",
								})({
									...(row.agent_id === null ? {} : { agent_id: row.agent_id }),
									causation_id: row.causation_id,
									correlation_id: row.correlation_id,
									journal_sequence: row.sequence,
									kind: "event",
									message_id: row.event_id,
									origin: row.origin,
									payload,
									protocol_version: 1,
									...(raw_origin === undefined ? {} : { raw_origin }),
									...(row.run_id === null ? {} : { run_id: row.run_id }),
									schema_version: row.schema_version,
									sent_at: row.occurred_at,
									sequence: row.stream_sequence,
									stream_id: row.stream_id,
									thread_id: row.thread_id,
								}),
							),
							Effect.mapError((error) =>
								error instanceof JournalInvariantError
									? error
									: new JournalInvariantError({
											message: `Affinity event ${row.event_id} is invalid`,
										}),
							),
						);
					}),
				);

		const RecordCommand = (
			transaction: ThreadTransaction,
			command: CommandEnvelope,
			accepted_at: string,
		) =>
			transaction.insert(JournalCommands).values({
				accepted_at,
				agent_id: command.agent_id ?? null,
				causation_id: command.causation_id ?? null,
				message_id: command.message_id,
				origin: command.origin,
				payload_json: JSON.stringify(command.payload),
				payload_type: command.payload.type,
				raw_origin_json: command.raw_origin ? JSON.stringify(command.raw_origin) : null,
				run_id: command.run_id ?? null,
				schema_version: command.schema_version,
				sent_at: command.sent_at,
				status: "accepted",
				thread_id: command.thread_id,
			});

		const Accept = (command: CommandEnvelope) =>
			Effect.gen(function* () {
				const payload = command.payload;

				if (!is_affinity_command_payload(payload)) {
					return yield* new JournalInvariantError({
						message: "Project affinity requires an assign or unlock command",
					});
				}

				const accepted = yield* database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const [existing] = yield* transaction
							.select()
							.from(JournalCommands)
							.where(eq(JournalCommands.message_id, command.message_id))
							.limit(1);

						if (existing) {
							if (!command_matches(command, existing)) {
								return yield* new CommandIdConflict({
									message_id: command.message_id,
								});
							}

							return { _tag: "Duplicate" as const };
						}

						const thread = yield* ReadThread(transaction, command.thread_id);
						const current = yield* DecodeThreadProjection(thread);
						const occurred_at = yield* metadata.Now;
						let event_payload: EventPayload;
						let projection: ThreadListItem | undefined;

						if (payload.type === "thread.project.assign") {
							const [project_row] = yield* transaction
								.select()
								.from(Projects)
								.where(eq(Projects.project_id, payload.project_id))
								.limit(1);
							if (!project_row) {
								return yield* new JournalInvariantError({
									message: `Project ${payload.project_id} is not attached to Forge`,
								});
							}
							const project = yield* Schema.decodeUnknownEffect(ProjectRef)(
								project_row,
							).pipe(
								Effect.mapError(
									() =>
										new JournalInvariantError({
											message: `Project ${payload.project_id} is invalid`,
										}),
								),
							);
							const linked_projects = unique_projects(
								[
									...current.linked_projects,
									...(current.primary_project ? [current.primary_project] : []),
									...current.project_affinity_scores.map(
										(score) => score.project,
									),
								],
								project,
							);

							projection = yield* MakeProjection(
								current,
								{
									linked_projects,
									primary_project: project,
									project_affinity_scores: current.project_affinity_scores,
									project_locked: true,
								},
								occurred_at,
							);
							event_payload = {
								change: "assigned",
								thread: projection,
								type: "thread.project_affinity.updated",
							};
						} else if (payload.basis_affinity_version !== current.affinity_version) {
							event_payload = {
								basis_affinity_version: payload.basis_affinity_version,
								reason: "stale_basis",
								type: "thread.project_affinity.ignored",
							};
						} else {
							const evidence = yield* ReadEvidence(transaction, command.thread_id);
							const automatic = yield* BuildAutomaticProjection(
								current,
								evidence,
								occurred_at,
							);

							projection = automatic.projection;
							event_payload = {
								change: "unlocked",
								thread: projection,
								type: "thread.project_affinity.updated",
							};
						}

						yield* RecordCommand(transaction, command, occurred_at);

						if (projection) {
							yield* WriteProjection(transaction, thread, projection);
						}

						return {
							_tag: "Accepted" as const,
							event: yield* AppendEvent(transaction, {
								...(command.agent_id ? { agent_id: command.agent_id } : {}),
								causation_id: command.message_id,
								correlation_id: command.message_id,
								occurred_at,
								payload: event_payload,
								...(command.raw_origin ? { raw_origin: command.raw_origin } : {}),
								...(command.run_id ? { run_id: command.run_id } : {}),
								thread_id: command.thread_id,
							}),
						};
					}),
				);

				if (accepted._tag === "Duplicate") {
					return {
						event: yield* ReadEvent(command.message_id),
						status: "duplicate" as const,
					};
				}

				yield* notifier.Publish(accepted.event.journal_sequence);

				return { event: accepted.event, status: "accepted" as const };
			}).pipe(Effect.mapError(normalize_error));

		const ObserveEvidence = (input: ThreadProjectAffinityEvidenceInput) =>
			Effect.gen(function* () {
				const accepted = yield* database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const existing_rows = yield* transaction
							.select()
							.from(ThreadProjectAffinityEvidence)
							.where(
								or(
									eq(
										ThreadProjectAffinityEvidence.evidence_id,
										input.evidence_id,
									),
									and(
										eq(
											ThreadProjectAffinityEvidence.thread_id,
											input.thread_id,
										),
										eq(
											ThreadProjectAffinityEvidence.source_event_id,
											input.source_event_id,
										),
										eq(ThreadProjectAffinityEvidence.kind, input.kind),
										eq(
											ThreadProjectAffinityEvidence.project_id,
											input.project.project_id,
										),
									),
								),
							);
						const conflicting = existing_rows.find(
							(row) => !evidence_matches(input, row),
						);

						if (conflicting) {
							return yield* new CommandIdConflict({
								message_id: input.evidence_id,
							});
						}

						const duplicate = existing_rows[0];

						if (duplicate) {
							return {
								_tag: "Duplicate" as const,
								correlation_id: duplicate.evidence_id,
							};
						}

						const thread = yield* ReadThread(transaction, input.thread_id);
						const current = yield* DecodeThreadProjection(thread);

						if (input.basis_affinity_version > current.affinity_version) {
							return yield* new JournalInvariantError({
								message: `Affinity evidence ${input.evidence_id} is based on a future projection`,
							});
						}

						yield* transaction.insert(ThreadProjectAffinityEvidence).values({
							basis_affinity_version: input.basis_affinity_version,
							evidence_id: input.evidence_id,
							kind: input.kind,
							observed_at: input.observed_at,
							project_id: input.project.project_id,
							project_json: serialize_project(input.project),
							source_event_id: input.source_event_id,
							source_journal_sequence: input.source_journal_sequence,
							thread_id: input.thread_id,
						});

						const occurred_at = yield* metadata.Now;
						let event_payload: EventPayload;
						let projection: ThreadListItem | undefined;

						if (current.project_locked) {
							event_payload = {
								basis_affinity_version: input.basis_affinity_version,
								reason: "locked",
								type: "thread.project_affinity.ignored",
							};
						} else {
							const evidence = yield* ReadEvidence(transaction, input.thread_id);
							const automatic = yield* BuildAutomaticProjection(
								current,
								evidence,
								occurred_at,
							);

							projection = automatic.projection;
							event_payload = {
								change: automatic.change,
								thread: projection,
								type: "thread.project_affinity.updated",
							};
						}

						if (projection) {
							yield* WriteProjection(transaction, thread, projection);
						}

						return {
							_tag: "Accepted" as const,
							event: yield* AppendEvent(transaction, {
								causation_id: input.source_event_id,
								correlation_id: input.evidence_id,
								occurred_at,
								payload: event_payload,
								thread_id: input.thread_id,
							}),
						};
					}),
				);

				if (accepted._tag === "Duplicate") {
					return {
						event: yield* ReadEvent(accepted.correlation_id),
						status: "duplicate" as const,
					};
				}

				yield* notifier.Publish(accepted.event.journal_sequence);

				return { event: accepted.event, status: "accepted" as const };
			}).pipe(Effect.mapError(normalize_error));

		return { Accept, ObserveEvidence };
	}),
);
