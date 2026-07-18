import { and, eq, exists, inArray, lte, not, notInArray, or, sql } from "drizzle-orm";
import { Context, Data, Effect, Layer } from "effect";

import { Database } from "../persistence/database";
import { JournalNotifier } from "../persistence/journal-notifier";
import { RetrySqliteWrite } from "../persistence/sqlite-write-retry";
import {
	AgentInstances,
	AgentRuns,
	ArtisanToolApprovals,
	ArtisanToolInvocations,
	Assignments,
	EventStreams,
	GitMutationOperations,
	JournalCommands,
	JournalEvents,
	OrchestrationArtifacts,
	OrchestrationCoordinators,
	OrchestrationGraphCommands,
	OrchestrationGraphEdges,
	OrchestrationGroups,
	OrchestrationInteractions,
	OrchestrationJoins,
	OrchestrationMessages,
	OrchestrationOutbox,
	OrchestrationRawObservations,
	OrchestrationRuns,
	TerminalCommands,
	TerminalSessions,
	ThreadErasureClaims,
	Threads,
	ThreadTombstones,
	WorkspaceChangeOperations,
	WorkspaceChangeDiffs,
	WorkspaceChanges,
	WorkspaceChangeSnapshots,
	WorkspaceMutationAuthorities,
	WorkspaceMutationPayloads,
} from "../persistence/schema";
import { ThreadResourceQuiescer } from "./thread-resource-quiescer";

/** Wraps an unexpected claim, resource-quiescence, or durable erasure failure. */
export class ThreadErasureFailure extends Data.TaggedError("ThreadErasureFailure")<{
	readonly cause: unknown;
}> {}

/** Claims expired threads and deeply erases every durable thread-owned row. */
export class ThreadErasure extends Context.Service<
	ThreadErasure,
	{
		readonly CleanupExpired: (
			cutoff: string,
			deleted_at: string,
		) => Effect.Effect<ReadonlyArray<string>, ThreadErasureFailure>;
		readonly ResumeClaimed: (
			deleted_at: string,
		) => Effect.Effect<ReadonlyArray<string>, ThreadErasureFailure>;
	}
>()("Artisan/ThreadErasure") {}

export const ThreadErasureLive = Layer.effect(
	ThreadErasure,
	Effect.gen(function* () {
		const database = yield* Database;
		const notifier = yield* JournalNotifier;
		const resources = yield* ThreadResourceQuiescer;

		const ReadClaims = database.client
			.select({ thread_id: ThreadErasureClaims.thread_id })
			.from(ThreadErasureClaims)
			.orderBy(ThreadErasureClaims.claimed_at, ThreadErasureClaims.thread_id)
			.pipe(Effect.map((rows) => rows.map((row) => row.thread_id)));

		const IsPendingWorkspaceMutation = or(
			notInArray(WorkspaceChangeOperations.lifecycle, ["claimed", "committed", "rejected"]),
			exists(
				database.client
					.select({ message_id: WorkspaceMutationPayloads.message_id })
					.from(WorkspaceMutationPayloads)
					.where(
						and(
							eq(
								WorkspaceMutationPayloads.message_id,
								WorkspaceChangeOperations.message_id,
							),
							or(
								eq(WorkspaceMutationPayloads.state, "available"),
								eq(WorkspaceChangeOperations.lifecycle, "claimed"),
							),
						),
					),
			),
		);
		const HasPendingGitMutation = (thread_id: typeof Threads.thread_id) =>
			exists(
				database.client
					.select({ mutation_id: GitMutationOperations.mutation_id })
					.from(GitMutationOperations)
					.where(
						and(
							eq(GitMutationOperations.thread_id, thread_id),
							inArray(GitMutationOperations.lifecycle, [
								"awaiting_approval",
								"approved",
								"dispatching",
							]),
						),
					),
			);
		const HasPendingToolInvocation = (thread_id: typeof Threads.thread_id) =>
			exists(
				database.client
					.select({ invocation_id: ArtisanToolInvocations.invocation_id })
					.from(ArtisanToolInvocations)
					.where(
						and(
							eq(ArtisanToolInvocations.thread_id, thread_id),
							notInArray(ArtisanToolInvocations.lifecycle, [
								"succeeded",
								"denied",
								"failed",
								"cancelled",
								"unsupported",
							]),
						),
					),
			);

		const ClaimExpired = (cutoff: string, claimed_at: string) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const candidates = yield* transaction
							.select({ thread_id: Threads.thread_id })
							.from(Threads)
							.where(
								and(
									eq(Threads.pinned, false),
									lte(Threads.last_activity_at, cutoff),
									not(
										exists(
											database.client
												.select({
													message_id:
														WorkspaceChangeOperations.message_id,
												})
												.from(WorkspaceChangeOperations)
												.where(
													and(
														eq(
															WorkspaceChangeOperations.thread_id,
															Threads.thread_id,
														),
														IsPendingWorkspaceMutation,
													),
												),
										),
									),
									not(HasPendingGitMutation(Threads.thread_id)),
									not(HasPendingToolInvocation(Threads.thread_id)),
								),
							)
							.orderBy(Threads.last_activity_at, Threads.thread_id);

						yield* Effect.forEach(
							candidates,
							(candidate) =>
								transaction
									.insert(ThreadErasureClaims)
									.values({ claimed_at, thread_id: candidate.thread_id })
									.onConflictDoNothing()
									.pipe(Effect.asVoid),
							{ discard: true },
						);
					}),
				)
				.pipe(RetrySqliteWrite);

		const EraseClaimed = (thread_id: string, deleted_at: string) =>
			Effect.gen(function* () {
				yield* resources.Quiesce(thread_id);

				const ErasureTransaction = database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const [claim] = yield* transaction
							.select({ thread_id: ThreadErasureClaims.thread_id })
							.from(ThreadErasureClaims)
							.where(eq(ThreadErasureClaims.thread_id, thread_id))
							.limit(1);

						if (!claim) {
							return undefined;
						}

						const [pending_mutation] = yield* transaction
							.select({ message_id: WorkspaceChangeOperations.message_id })
							.from(WorkspaceChangeOperations)
							.where(
								and(
									eq(WorkspaceChangeOperations.thread_id, thread_id),
									IsPendingWorkspaceMutation,
								),
							)
							.limit(1);
						const [pending_git_mutation] = yield* transaction
							.select({ mutation_id: GitMutationOperations.mutation_id })
							.from(GitMutationOperations)
							.where(
								and(
									eq(GitMutationOperations.thread_id, thread_id),
									inArray(GitMutationOperations.lifecycle, [
										"awaiting_approval",
										"approved",
										"dispatching",
									]),
								),
							)
							.limit(1);
						const [pending_tool_invocation] = yield* transaction
							.select({ invocation_id: ArtisanToolInvocations.invocation_id })
							.from(ArtisanToolInvocations)
							.where(
								and(
									eq(ArtisanToolInvocations.thread_id, thread_id),
									notInArray(ArtisanToolInvocations.lifecycle, [
										"succeeded",
										"denied",
										"failed",
										"cancelled",
										"unsupported",
									]),
								),
							)
							.limit(1);

						if (pending_mutation || pending_git_mutation || pending_tool_invocation) {
							yield* transaction
								.delete(ThreadErasureClaims)
								.where(eq(ThreadErasureClaims.thread_id, thread_id));

							return undefined;
						}

						const stream_id = `thread:${thread_id}`;
						const [stream] = yield* transaction
							.select({ last_sequence: EventStreams.last_sequence })
							.from(EventStreams)
							.where(eq(EventStreams.stream_id, stream_id))
							.limit(1);

						if (!stream) {
							return yield* new ThreadErasureFailure({
								cause: new Error(`Thread ${thread_id} is missing its event stream`),
							});
						}

						const stream_sequence = stream.last_sequence + 1;
						const erasure_id = `thread_erased_${thread_id}`;

						const groups = yield* transaction
							.select({ group_id: OrchestrationGroups.group_id })
							.from(OrchestrationGroups)
							.where(eq(OrchestrationGroups.thread_id, thread_id));
						const group_ids = groups.map((group) => group.group_id);
						const base_runs = yield* transaction
							.select({ run_id: OrchestrationRuns.run_id })
							.from(OrchestrationRuns)
							.where(eq(OrchestrationRuns.thread_id, thread_id));
						const graph_runs =
							group_ids.length === 0
								? []
								: yield* transaction
										.select({ run_id: AgentRuns.run_id })
										.from(AgentRuns)
										.where(inArray(AgentRuns.group_id, group_ids));
						const run_ids = [
							...base_runs.map((run) => run.run_id),
							...graph_runs.map((run) => run.run_id),
						];
						const terminal_rows = yield* transaction
							.select({ terminal_id: TerminalSessions.terminal_id })
							.from(TerminalSessions)
							.where(eq(TerminalSessions.thread_id, thread_id));
						const terminal_ids = terminal_rows.map((terminal) => terminal.terminal_id);
						const tool_invocation_rows = yield* transaction
							.select({ invocation_id: ArtisanToolInvocations.invocation_id })
							.from(ArtisanToolInvocations)
							.where(eq(ArtisanToolInvocations.thread_id, thread_id));
						const tool_invocation_ids = tool_invocation_rows.map(
							(invocation) => invocation.invocation_id,
						);

						if (run_ids.length > 0) {
							yield* transaction
								.delete(OrchestrationRawObservations)
								.where(inArray(OrchestrationRawObservations.run_id, run_ids));
							yield* transaction
								.delete(OrchestrationInteractions)
								.where(inArray(OrchestrationInteractions.run_id, run_ids));
						}

						if (group_ids.length > 0) {
							yield* transaction
								.delete(OrchestrationArtifacts)
								.where(inArray(OrchestrationArtifacts.group_id, group_ids));
							yield* transaction
								.delete(OrchestrationGraphEdges)
								.where(inArray(OrchestrationGraphEdges.group_id, group_ids));
							yield* transaction
								.delete(OrchestrationJoins)
								.where(inArray(OrchestrationJoins.group_id, group_ids));
							yield* transaction
								.delete(OrchestrationGraphCommands)
								.where(inArray(OrchestrationGraphCommands.group_id, group_ids));
							yield* transaction
								.delete(AgentRuns)
								.where(inArray(AgentRuns.group_id, group_ids));
							yield* transaction
								.delete(Assignments)
								.where(inArray(Assignments.group_id, group_ids));
							yield* transaction
								.delete(AgentInstances)
								.where(inArray(AgentInstances.group_id, group_ids));
							yield* transaction
								.delete(OrchestrationGroups)
								.where(inArray(OrchestrationGroups.group_id, group_ids));
						}

						if (terminal_ids.length > 0) {
							yield* transaction
								.delete(TerminalCommands)
								.where(inArray(TerminalCommands.terminal_id, terminal_ids));
						}

						yield* transaction
							.delete(TerminalSessions)
							.where(eq(TerminalSessions.thread_id, thread_id));
						yield* transaction
							.delete(OrchestrationOutbox)
							.where(eq(OrchestrationOutbox.thread_id, thread_id));
						yield* transaction
							.delete(OrchestrationMessages)
							.where(eq(OrchestrationMessages.thread_id, thread_id));
						yield* transaction
							.delete(OrchestrationCoordinators)
							.where(eq(OrchestrationCoordinators.thread_id, thread_id));
						yield* transaction
							.delete(OrchestrationRuns)
							.where(eq(OrchestrationRuns.thread_id, thread_id));
						yield* transaction
							.delete(WorkspaceChangeDiffs)
							.where(eq(WorkspaceChangeDiffs.thread_id, thread_id));
						yield* transaction
							.delete(WorkspaceChanges)
							.where(eq(WorkspaceChanges.thread_id, thread_id));
						yield* transaction
							.delete(WorkspaceChangeSnapshots)
							.where(eq(WorkspaceChangeSnapshots.thread_id, thread_id));
						yield* transaction
							.delete(WorkspaceMutationPayloads)
							.where(eq(WorkspaceMutationPayloads.thread_id, thread_id));
						yield* transaction
							.delete(WorkspaceMutationAuthorities)
							.where(eq(WorkspaceMutationAuthorities.thread_id, thread_id));
						yield* transaction
							.delete(WorkspaceChangeOperations)
							.where(eq(WorkspaceChangeOperations.thread_id, thread_id));
						yield* transaction
							.delete(GitMutationOperations)
							.where(eq(GitMutationOperations.thread_id, thread_id));
						if (tool_invocation_ids.length > 0) {
							yield* transaction
								.delete(ArtisanToolApprovals)
								.where(
									inArray(
										ArtisanToolApprovals.invocation_id,
										tool_invocation_ids,
									),
								);
						}
						yield* transaction
							.delete(ArtisanToolInvocations)
							.where(eq(ArtisanToolInvocations.thread_id, thread_id));
						yield* transaction
							.delete(JournalCommands)
							.where(eq(JournalCommands.thread_id, thread_id));

						yield* transaction
							.update(JournalEvents)
							.set({
								agent_id: null,
								causation_id: sql`'thread_content_erased_' || ${JournalEvents.sequence}`,
								correlation_id: sql`'thread_content_erased_' || ${JournalEvents.sequence}`,
								event_id: sql`'thread_content_erased_' || ${JournalEvents.sequence}`,
								event_type: "thread.content_erased",
								occurred_at: "1970-01-01T00:00:00.000Z",
								origin: "backend",
								payload_json: '{"type":"thread.content_erased"}',
								raw_origin_json: null,
								run_id: null,
								schema_version: 1,
							})
							.where(eq(JournalEvents.thread_id, thread_id));

						yield* transaction
							.update(EventStreams)
							.set({ last_sequence: stream_sequence })
							.where(eq(EventStreams.stream_id, stream_id));

						const [erasure_event] = yield* transaction
							.insert(JournalEvents)
							.values({
								agent_id: null,
								causation_id: erasure_id,
								correlation_id: erasure_id,
								event_id: erasure_id,
								event_type: "thread.erased",
								occurred_at: deleted_at,
								origin: "backend",
								payload_json: '{"type":"thread.erased"}',
								raw_origin_json: null,
								run_id: null,
								schema_version: 1,
								stream_id,
								stream_sequence,
								thread_id,
							})
							.returning({ journal_sequence: JournalEvents.sequence });

						yield* transaction
							.insert(ThreadTombstones)
							.values({ deleted_at, thread_id })
							.onConflictDoNothing();
						yield* transaction.delete(Threads).where(eq(Threads.thread_id, thread_id));
						yield* transaction
							.delete(ThreadErasureClaims)
							.where(eq(ThreadErasureClaims.thread_id, thread_id));

						return erasure_event!.journal_sequence;
					}),
				);
				const journal_sequence = yield* RetrySqliteWrite(ErasureTransaction);

				if (journal_sequence === undefined) {
					return false;
				}

				yield* notifier.Publish(journal_sequence);

				return true;
			});

		const DrainClaims = (deleted_at: string) =>
			Effect.gen(function* () {
				const claims = yield* ReadClaims;
				const outcomes = yield* Effect.forEach(
					claims,
					(thread_id) =>
						EraseClaimed(thread_id, deleted_at).pipe(
							Effect.map((erased) => ({ erased, thread_id })),
						),
					{ concurrency: 1 },
				);

				return outcomes
					.filter((outcome) => outcome.erased)
					.map((outcome) => outcome.thread_id);
			});

		return {
			CleanupExpired: (cutoff, deleted_at) =>
				ClaimExpired(cutoff, deleted_at).pipe(
					Effect.andThen(DrainClaims(deleted_at)),
					Effect.mapError((cause) => new ThreadErasureFailure({ cause })),
				),
			ResumeClaimed: (deleted_at) =>
				DrainClaims(deleted_at).pipe(
					Effect.mapError((cause) => new ThreadErasureFailure({ cause })),
				),
		};
	}),
);
