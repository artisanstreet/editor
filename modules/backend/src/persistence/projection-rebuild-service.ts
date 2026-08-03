import { asc, eq, inArray, sql } from "drizzle-orm";
import { Context, Data, Effect, Layer, Schema } from "effect";

import {
	EventPayload,
	type GitWorkspaceProjection,
	type ThreadListItem,
	type WorkspaceChange,
} from "@artisan/protocol";

import { Database } from "./database";
import { RetrySqliteWrite } from "./sqlite-write-retry";
import {
	EventStreams,
	GitMutationOperations,
	GitWorkspaceProjections,
	JournalEvents,
	LegacyWorkspaceChangeProjections,
	ProjectionRebuildLocks,
	ThreadErasureClaims,
	Threads,
	WorkspaceChangeDiffs,
	WorkspaceChangeOperations,
	WorkspaceChanges,
} from "./tables";
import {
	automatic_thread_title_from_event,
	thread_activity_kind_from_event,
} from "../threads/internal/thread-activity";

/**
 * Effect's experimental EventLog owns a separate replicated SQL journal, identity,
 * and entry model. Artisan's existing journal has immutable global and per-stream
 * cursors, raw-origin redaction, and Drizzle-owned cross-domain transactions, so a
 * migration to it is not a safe projection-rebuild implementation.
 */

export class ProjectionRebuildBusy extends Data.TaggedError("ProjectionRebuildBusy")<{
	readonly reason: string;
}> {}

export class ProjectionRebuildInvariantError extends Data.TaggedError(
	"ProjectionRebuildInvariantError",
)<{
	readonly message: string;
}> {}

export class ProjectionRebuildFailure extends Data.TaggedError("ProjectionRebuildFailure")<{
	readonly cause: unknown;
}> {}

export type ProjectionRebuildError =
	| ProjectionRebuildBusy
	| ProjectionRebuildInvariantError
	| ProjectionRebuildFailure;

/** Equivalence result for the three supported public projections only. */
export interface PublicProjectionRebuildVerification {
	readonly equivalent: boolean;
	readonly git_workspace_count: number;
	readonly journal_sequence: number;
	readonly thread_count: number;
	readonly workspace_change_count: number;
}

/** Repair result for threads, workspace changes, and Git workspaces only. */
export interface PublicProjectionRebuildResult extends PublicProjectionRebuildVerification {
	readonly rebuilt: boolean;
}

export class ProjectionRebuildService extends Context.Service<
	ProjectionRebuildService,
	{
		readonly Rebuild: () => Effect.Effect<
			PublicProjectionRebuildResult,
			ProjectionRebuildError
		>;
		readonly Verify: () => Effect.Effect<
			PublicProjectionRebuildVerification,
			ProjectionRebuildError
		>;
	}
>()("Artisan/ProjectionRebuildService") {}

/** Internal deterministic test seam; production supplies the no-op implementation. */
export class ProjectionRebuildBarrier extends Context.Service<
	ProjectionRebuildBarrier,
	{
		readonly BeforeWriterLock: Effect.Effect<void>;
		readonly AfterWriterLock: Effect.Effect<void>;
	}
>()("Artisan/ProjectionRebuildBarrier") {}

export const ProjectionRebuildBarrierLive = Layer.succeed(ProjectionRebuildBarrier, {
	BeforeWriterLock: Effect.void,
	AfterWriterLock: Effect.void,
});

type RebuiltThread = typeof Threads.$inferInsert;
type RebuiltWorkspaceChange = typeof WorkspaceChanges.$inferInsert;
type RebuiltGitWorkspace = typeof GitWorkspaceProjections.$inferInsert;

interface RebuiltProjections {
	readonly git_workspaces: ReadonlyArray<RebuiltGitWorkspace>;
	readonly journal_sequence: number;
	readonly threads: ReadonlyArray<RebuiltThread>;
	readonly workspace_changes: ReadonlyArray<RebuiltWorkspaceChange>;
}

const stable_json = (value: unknown): string => {
	if (Array.isArray(value)) {
		return `[${value.map(stable_json).join(",")}]`;
	}

	if (value !== null && typeof value === "object") {
		const record = value as Record<string, unknown>;

		return `{${Object.keys(record)
			.sort()
			.map((key) => `${JSON.stringify(key)}:${stable_json(record[key])}`)
			.join(",")}}`;
	}

	return JSON.stringify(value);
};

const project_json = (project: ThreadListItem["primary_project"]) =>
	project === undefined ? null : JSON.stringify(project);

const thread_row = (thread: ThreadListItem): RebuiltThread => ({
	activity_version: thread.activity_version,
	affinity_version: thread.affinity_version,
	archived_at: thread.archived_at ?? null,
	created_at: thread.created_at,
	current_goal: thread.current_goal ?? null,
	last_activity_at: thread.last_activity_at,
	linked_projects_json: JSON.stringify(thread.linked_projects),
	live_status: thread.live_status,
	metadata_version: thread.metadata_version,
	pinned: thread.pinned,
	primary_project_id: thread.primary_project?.project_id ?? null,
	primary_project_json: project_json(thread.primary_project),
	project_affinity_scores_json: JSON.stringify(thread.project_affinity_scores),
	project_locked: thread.project_locked,
	rehome_suggestion_json: thread.rehome_suggestion
		? JSON.stringify(thread.rehome_suggestion)
		: null,
	rename_suggestion: thread.rename_suggestion ?? null,
	thread_id: thread.thread_id,
	title: thread.title,
	title_locked: thread.title_locked,
	title_source: thread.title_source,
	updated_at: thread.updated_at,
});

const created_thread_row = (thread_id: string, title: string, occurred_at: string): RebuiltThread =>
	thread_row({
		affinity_version: 0,
		created_at: occurred_at,
		last_activity_at: occurred_at,
		live_status: "Idle",
		metadata_version: 0,
		pinned: false,
		project_affinity_scores: [],
		project_locked: false,
		linked_projects: [],
		thread_id,
		title,
		title_locked: false,
		title_source: "initial",
		updated_at: occurred_at,
		activity_version: 0,
		current_goal: title,
	});

const workspace_change_row = (
	change: WorkspaceChange,
	diff_state: "available" | "legacy_unavailable",
): RebuiltWorkspaceChange => ({
	after_identity_json: JSON.stringify(change.after_identity),
	agent_id: change.agent_id,
	before_identity_json: JSON.stringify(change.before_identity),
	change_id: change.change_id,
	created_at: change.created_at,
	diff_state,
	path: change.path,
	raw_origin_json: change.raw_origin ? JSON.stringify(change.raw_origin) : null,
	review_state: change.review_state,
	review_source_command_id: change.review?.source_command_id ?? null,
	reviewer_agent_id: change.review?.reviewer_agent_id ?? null,
	reviewer_assignment_id: change.review?.assignment_id ?? null,
	reviewer_group_id: change.review?.group_id ?? null,
	reviewer_kind: change.review?.reviewer_kind ?? null,
	reviewer_raw_origin_json: change.review?.raw_origin
		? JSON.stringify(change.review.raw_origin)
		: null,
	reviewer_run_id: change.review?.reviewer_run_id ?? null,
	review_outcome: change.review?.outcome ?? null,
	review_comment: change.review?.comment ?? null,
	reviewed_at: change.reviewed_at ?? null,
	rollback_state: change.rollback_state,
	rolled_back_at: change.rolled_back_at ?? null,
	run_id: change.run_id,
	source_command_id: change.source_command_id,
	thread_id: change.thread_id,
	updated_at: change.updated_at,
	version: change.version,
	workspace_id: change.workspace_id,
});

const git_workspace_row = (
	workspace: GitWorkspaceProjection,
	updated_at: string,
): RebuiltGitWorkspace => ({
	journal_sequence: workspace.journal_sequence,
	observed_at: workspace.observed_at,
	projection_json: JSON.stringify(workspace),
	snapshot_id: workspace.snapshot_id,
	updated_at,
	version: workspace.version,
	workspace_id: workspace.workspace_id,
});

const normalize_error = (error: unknown): ProjectionRebuildError =>
	error instanceof ProjectionRebuildBusy || error instanceof ProjectionRebuildInvariantError
		? error
		: new ProjectionRebuildFailure({ cause: error });

export const ProjectionRebuildServiceLive = Layer.effect(
	ProjectionRebuildService,
	Effect.gen(function* () {
		const database = yield* Database;
		const barrier = yield* ProjectionRebuildBarrier;

		const AssertIdle = (transaction: typeof database.client) =>
			Effect.gen(function* () {
				const [erasure_claim] = yield* transaction
					.select({ thread_id: ThreadErasureClaims.thread_id })
					.from(ThreadErasureClaims)
					.limit(1);
				if (erasure_claim) {
					return yield* new ProjectionRebuildBusy({
						reason: "A thread erasure claim is active.",
					});
				}

				const [workspace_mutation] = yield* transaction
					.select({ message_id: WorkspaceChangeOperations.message_id })
					.from(WorkspaceChangeOperations)
					.where(inArray(WorkspaceChangeOperations.lifecycle, ["claimed", "applied"]))
					.limit(1);
				if (workspace_mutation) {
					return yield* new ProjectionRebuildBusy({
						reason: "A workspace mutation is active.",
					});
				}

				const [git_dispatch] = yield* transaction
					.select({ mutation_id: GitMutationOperations.mutation_id })
					.from(GitMutationOperations)
					.where(inArray(GitMutationOperations.lifecycle, ["approved", "dispatching"]))
					.limit(1);
				if (git_dispatch) {
					return yield* new ProjectionRebuildBusy({
						reason: "A Git mutation dispatch is active.",
					});
				}
			});

		const Derive = (transaction: typeof database.client) =>
			Effect.gen(function* () {
				const events = yield* transaction
					.select()
					.from(JournalEvents)
					.orderBy(asc(JournalEvents.sequence));
				const stream_rows = yield* transaction
					.select()
					.from(EventStreams)
					.orderBy(asc(EventStreams.stream_id));
				const diff_rows = yield* transaction
					.select({ change_id: WorkspaceChangeDiffs.change_id })
					.from(WorkspaceChangeDiffs);
				const available_diffs = new Set(diff_rows.map((row) => row.change_id));
				const legacy_change_rows = yield* transaction
					.select({
						change_id: LegacyWorkspaceChangeProjections.change_id,
						source_command_id: LegacyWorkspaceChangeProjections.source_command_id,
					})
					.from(LegacyWorkspaceChangeProjections);
				const legacy_changes = new Map(
					legacy_change_rows.map((row) => [row.change_id, row.source_command_id]),
				);
				const threads = new Map<string, RebuiltThread>();
				const workspace_changes = new Map<string, RebuiltWorkspaceChange>();
				const git_workspaces = new Map<string, RebuiltGitWorkspace>();
				const erased_threads = new Set<string>();
				const streams = new Map<string, number>();
				let expected_sequence = 1;

				for (const event of events) {
					if (event.sequence !== expected_sequence) {
						return yield* new ProjectionRebuildInvariantError({
							message: "Journal global sequence is not contiguous.",
						});
					}
					expected_sequence += 1;
					const expected_stream_sequence = (streams.get(event.stream_id) ?? 0) + 1;
					if (event.stream_sequence !== expected_stream_sequence) {
						return yield* new ProjectionRebuildInvariantError({
							message: `Journal stream ${event.stream_id} is not contiguous.`,
						});
					}
					streams.set(event.stream_id, event.stream_sequence);

					const payload_json = yield* Schema.decodeUnknownEffect(
						Schema.UnknownFromJsonString,
					)(event.payload_json).pipe(
						Effect.mapError(
							() =>
								new ProjectionRebuildInvariantError({
									message: `Journal event ${event.event_id} contains invalid JSON.`,
								}),
						),
					);
					const payload = yield* Schema.decodeUnknownEffect(EventPayload, {
						onExcessProperty: "error",
					})(payload_json).pipe(
						Effect.mapError(
							() =>
								new ProjectionRebuildInvariantError({
									message: `Journal event ${event.event_id} does not match EventPayload.`,
								}),
						),
					);
					if (payload.type !== event.event_type) {
						return yield* new ProjectionRebuildInvariantError({
							message: `Journal event ${event.event_id} type does not match its payload.`,
						});
					}

					switch (payload.type) {
						case "thread.created":
							if (
								erased_threads.has(event.thread_id) ||
								threads.has(event.thread_id)
							) {
								return yield* new ProjectionRebuildInvariantError({
									message: `Thread ${event.thread_id} has an invalid creation history.`,
								});
							}
							threads.set(
								event.thread_id,
								created_thread_row(
									event.thread_id,
									payload.title,
									event.occurred_at,
								),
							);
							break;
						case "thread.metadata.updated":
						case "thread.project_affinity.updated":
							if (
								erased_threads.has(event.thread_id) ||
								!threads.has(event.thread_id)
							) {
								return yield* new ProjectionRebuildInvariantError({
									message: `Thread ${event.thread_id} was updated without a live creation.`,
								});
							}
							threads.set(event.thread_id, thread_row(payload.thread));
							break;
						case "thread.erased":
							erased_threads.add(event.thread_id);
							threads.delete(event.thread_id);
							for (const [change_id, change] of workspace_changes) {
								if (change.thread_id === event.thread_id)
									workspace_changes.delete(change_id);
							}
							break;
						case "workspace.change.updated":
							if (!erased_threads.has(payload.change.thread_id)) {
								const has_diff = available_diffs.has(payload.change.change_id);
								const is_legacy =
									legacy_changes.get(payload.change.change_id) ===
									payload.change.source_command_id;
								if (!has_diff && !is_legacy) {
									return yield* new ProjectionRebuildInvariantError({
										message: `Workspace change ${payload.change.change_id} has no retained immutable diff.`,
									});
								}
								workspace_changes.set(
									payload.change.change_id,
									workspace_change_row(
										payload.change,
										has_diff ? "available" : "legacy_unavailable",
									),
								);
							}
							break;
						case "git.workspace.updated":
							if (payload.workspace.journal_sequence !== event.sequence) {
								return yield* new ProjectionRebuildInvariantError({
									message: `Git workspace ${payload.workspace.workspace_id} does not reference its journal event.`,
								});
							}
							git_workspaces.set(
								payload.workspace.workspace_id,
								git_workspace_row(payload.workspace, event.occurred_at),
							);
							break;
					}

					const activity_kind = thread_activity_kind_from_event(payload);
					const active_thread = threads.get(event.thread_id);
					if (activity_kind !== undefined && active_thread !== undefined) {
						const automatic_title = automatic_thread_title_from_event(payload);
						const updates_title =
							automatic_title !== undefined &&
							!active_thread.title_locked &&
							(active_thread.title !== automatic_title ||
								active_thread.title_source !== "automatic");

						threads.set(event.thread_id, {
							...active_thread,
							activity_version: (active_thread.activity_version ?? 0) + 1,
							last_activity_at:
								(active_thread.last_activity_at ?? "1970-01-01T00:00:00.000Z") >
								event.occurred_at
									? active_thread.last_activity_at
									: event.occurred_at,
							updated_at:
								active_thread.updated_at > event.occurred_at
									? active_thread.updated_at
									: event.occurred_at,
							...(updates_title
								? {
										metadata_version: (active_thread.metadata_version ?? 0) + 1,
										title: automatic_title,
										title_source: "automatic" as const,
									}
								: {}),
						});
					}
				}

				if (stream_rows.length !== streams.size) {
					return yield* new ProjectionRebuildInvariantError({
						message: "Event stream rows do not match the journal.",
					});
				}
				for (const stream of stream_rows) {
					if (streams.get(stream.stream_id) !== stream.last_sequence) {
						return yield* new ProjectionRebuildInvariantError({
							message: `Event stream ${stream.stream_id} cursor does not match the journal.`,
						});
					}
				}

				return {
					git_workspaces: [...git_workspaces.values()].sort((left, right) =>
						left.workspace_id.localeCompare(right.workspace_id),
					),
					journal_sequence: events.at(-1)?.sequence ?? 0,
					threads: [...threads.values()].sort((left, right) =>
						left.thread_id.localeCompare(right.thread_id),
					),
					workspace_changes: [...workspace_changes.values()].sort((left, right) =>
						left.change_id.localeCompare(right.change_id),
					),
				} satisfies RebuiltProjections;
			});

		const VerifyDerived = (transaction: typeof database.client, derived: RebuiltProjections) =>
			Effect.gen(function* () {
				const [stored_threads, stored_workspace_changes, stored_git_workspaces] =
					yield* Effect.all([
						transaction.select().from(Threads).orderBy(asc(Threads.thread_id)),
						transaction
							.select()
							.from(WorkspaceChanges)
							.orderBy(asc(WorkspaceChanges.change_id)),
						transaction
							.select()
							.from(GitWorkspaceProjections)
							.orderBy(asc(GitWorkspaceProjections.workspace_id)),
					]);
				const threads_equivalent =
					stable_json(stored_threads) === stable_json(derived.threads);
				const workspace_changes_equivalent =
					stable_json(stored_workspace_changes) ===
					stable_json(derived.workspace_changes);
				const git_workspaces_equivalent =
					stable_json(stored_git_workspaces) === stable_json(derived.git_workspaces);
				const equivalent =
					threads_equivalent && workspace_changes_equivalent && git_workspaces_equivalent;

				return {
					equivalent,
					git_workspace_count: derived.git_workspaces.length,
					journal_sequence: derived.journal_sequence,
					thread_count: derived.threads.length,
					workspace_change_count: derived.workspace_changes.length,
				} satisfies PublicProjectionRebuildVerification;
			});

		const Verify = () =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						yield* AssertIdle(transaction);
						const derived = yield* Derive(transaction);
						return yield* VerifyDerived(transaction, derived);
					}),
				)
				.pipe(Effect.mapError(normalize_error));

		const Rebuild = () =>
			barrier.BeforeWriterLock.pipe(
				Effect.andThen(() =>
					database.client
						.transaction((transaction) =>
							Effect.gen(function* () {
								/**
								 * Drizzle holds Effect SqlClient.withTransaction's connection semaphore for
								 * this complete callback. Incrementing the singleton generation obtains
								 * SQLite's cross-process writer reservation before the journal snapshot.
								 */
								const locked = yield* transaction
									.update(ProjectionRebuildLocks)
									.set({
										generation: sql`${ProjectionRebuildLocks.generation} + 1`,
									})
									.where(eq(ProjectionRebuildLocks.lock_id, 1))
									.returning({ lock_id: ProjectionRebuildLocks.lock_id });
								if (locked.length !== 1) {
									return yield* new ProjectionRebuildInvariantError({
										message: "Projection rebuild writer lock is missing.",
									});
								}
								yield* barrier.AfterWriterLock;
								yield* AssertIdle(transaction);
								const derived = yield* Derive(transaction);
								const verified = yield* VerifyDerived(transaction, derived);

								if (!verified.equivalent) {
									const existing_changes = yield* transaction
										.select({ change_id: WorkspaceChanges.change_id })
										.from(WorkspaceChanges);
									const rebuilt_change_ids = new Set(
										derived.workspace_changes.map((row) => row.change_id),
									);
									const stale_change_ids = existing_changes
										.map((row) => row.change_id)
										.filter((change_id) => !rebuilt_change_ids.has(change_id));
									if (stale_change_ids.length > 0) {
										const retained_diffs = yield* transaction
											.select({ change_id: WorkspaceChangeDiffs.change_id })
											.from(WorkspaceChangeDiffs)
											.where(
												inArray(
													WorkspaceChangeDiffs.change_id,
													stale_change_ids,
												),
											);
										if (retained_diffs.length > 0) {
											return yield* new ProjectionRebuildInvariantError({
												message:
													"A stale workspace projection still owns an immutable diff.",
											});
										}
										yield* transaction
											.delete(WorkspaceChanges)
											.where(
												inArray(
													WorkspaceChanges.change_id,
													stale_change_ids,
												),
											);
									}
									yield* transaction.delete(GitWorkspaceProjections);
									yield* transaction.delete(Threads);
									if (derived.threads.length > 0)
										yield* transaction.insert(Threads).values(derived.threads);
									if (derived.workspace_changes.length > 0) {
										yield* Effect.forEach(derived.workspace_changes, (row) => {
											const { change_id: _change_id, ...values } = row;

											return transaction
												.insert(WorkspaceChanges)
												.values(row)
												.onConflictDoUpdate({
													target: WorkspaceChanges.change_id,
													set: values,
												});
										});
									}
									if (derived.git_workspaces.length > 0) {
										yield* transaction
											.insert(GitWorkspaceProjections)
											.values(derived.git_workspaces);
									}
									const repaired = yield* VerifyDerived(transaction, derived);
									if (!repaired.equivalent) {
										return yield* new ProjectionRebuildInvariantError({
											message:
												"Projection rebuild could not produce an equivalent projection set.",
										});
									}
								}

								return {
									...verified,
									equivalent: true,
									rebuilt: !verified.equivalent,
								} satisfies PublicProjectionRebuildResult;
							}),
						)
						.pipe(RetrySqliteWrite, Effect.mapError(normalize_error)),
				),
			);

		return { Rebuild, Verify };
	}),
);
