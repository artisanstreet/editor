import { and, asc, eq, inArray, isNull, lte, or } from "drizzle-orm";
import { Effect, Layer, Schema } from "effect";

import {
	GitMutationProjection,
	GitMutationUpdatedEvent,
	Identifier,
	git_workspace_maximum_pending_mutations,
} from "@artisan/protocol";

import { Database } from "../persistence/database";
import { JournalNotifier } from "../persistence/journal-notifier";
import { GitMutationOperations } from "../persistence/tables";
import { RetrySqliteWrite } from "../persistence/sqlite-write-retry";
import { RuntimeMetadata } from "../runtime/metadata";
import { GitRepository, GitRepositoryConflict, GitWorkspaceRecordInput } from "./contracts";

export * from "./contracts";
import { MakeGitMutationLifecycle } from "./mutation-lifecycle";
import { GitRuntime, MakeGitRuntime } from "./runtime";
/** Supplies the SQLite-backed Git repository. */
export const GitRepositoryLive = Layer.effect(
	GitRepository,
	Effect.gen(function* () {
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;
		const notifier = yield* JournalNotifier;
		const runtime = yield* MakeGitRuntime;
		const {
			AppendEvent,
			Decode,
			DecodeMutationRow,
			ReadMutationTransaction,
			ReadWorkspaceTransaction,
			RecordWorkspaceTransaction,
			EnsureLiveThread,
			invariant,
			normalize_error,
		} = runtime;
		const { ClaimApproved, CommitSucceeded, CommitTerminal, RequestMutation, ResolveMutation } =
			yield* MakeGitMutationLifecycle.pipe(Effect.provideService(GitRuntime, runtime));
		const RecordWorkspace = (input: GitWorkspaceRecordInput) =>
			Decode(GitWorkspaceRecordInput, input, "record_workspace").pipe(
				Effect.flatMap((decoded) =>
					RetrySqliteWrite(
						database.client.transaction((transaction) =>
							RecordWorkspaceTransaction(transaction, decoded),
						),
					),
				),
				Effect.mapError(normalize_error),
				Effect.tap((result) =>
					result.status === "accepted"
						? notifier.Publish(result.event.journal_sequence)
						: Effect.void,
				),
			);

		const ReadWorkspace = (input: string) =>
			Decode(Identifier, input, "read_workspace").pipe(
				Effect.flatMap((workspace_id) =>
					ReadWorkspaceTransaction(database.client, workspace_id),
				),
				Effect.mapError(normalize_error),
			);

		const ReadMutation = (input: string) =>
			Decode(Identifier, input, "read_mutation").pipe(
				Effect.flatMap((mutation_id) =>
					ReadMutationTransaction(database.client, mutation_id).pipe(
						Effect.map(({ projection }) => projection),
					),
				),
				Effect.mapError(normalize_error),
			);

		const ListPending = (input?: string) =>
			Effect.gen(function* () {
				const workspace_id =
					input === undefined
						? undefined
						: yield* Decode(Identifier, input, "list_pending");
				{
					const states = [
						"awaiting_approval",
						"approved",
						"dispatching",
						"ambiguous",
					] as const;
					const query =
						workspace_id === undefined
							? database.client
									.select()
									.from(GitMutationOperations)
									.where(inArray(GitMutationOperations.lifecycle, states))
							: database.client
									.select()
									.from(GitMutationOperations)
									.where(
										and(
											eq(GitMutationOperations.workspace_id, workspace_id),
											inArray(GitMutationOperations.lifecycle, states),
										),
									);

					return yield* query
						.orderBy(
							asc(GitMutationOperations.requested_at),
							asc(GitMutationOperations.mutation_id),
						)
						.pipe(
							Effect.flatMap((rows) =>
								Effect.forEach(rows, (row) =>
									DecodeMutationRow(row).pipe(
										Effect.map(({ projection }) => projection),
									),
								),
							),
							Effect.flatMap(
								Schema.decodeUnknownEffect(
									Schema.Array(GitMutationProjection).check(
										Schema.isMaxLength(git_workspace_maximum_pending_mutations),
									),
									{ onExcessProperty: "error" },
								),
							),
							Effect.mapError(() =>
								invariant("Pending Git mutation list is invalid"),
							),
						);
				}
			}).pipe(Effect.mapError(normalize_error));

		const RecoverDispatching = () =>
			RetrySqliteWrite(
				database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const recovery_at = yield* metadata.Now;
						const rows = yield* transaction
							.select()
							.from(GitMutationOperations)
							.where(
								and(
									eq(GitMutationOperations.lifecycle, "dispatching"),
									or(
										isNull(GitMutationOperations.dispatch_lease_expires_at),
										lte(
											GitMutationOperations.dispatch_lease_expires_at,
											recovery_at,
										),
									),
								),
							)
							.orderBy(
								asc(GitMutationOperations.requested_at),
								asc(GitMutationOperations.mutation_id),
							);
						const ambiguous = yield* Effect.forEach(rows, (row) =>
							Effect.gen(function* () {
								const mutation = yield* DecodeMutationRow(row);
								yield* EnsureLiveThread(transaction, mutation.row.thread_id);
								const failure = { code: "git_dispatch_recovery" } as const;
								const updated_at = recovery_at;
								const [updated] = yield* transaction
									.update(GitMutationOperations)
									.set({
										completed_at: updated_at,
										failure_code: JSON.stringify(failure),
										lifecycle: "ambiguous",
										updated_at,
									})
									.where(
										and(
											eq(
												GitMutationOperations.mutation_id,
												mutation.row.mutation_id,
											),
											eq(GitMutationOperations.lifecycle, "dispatching"),
										),
									)
									.returning();

								if (updated === undefined) {
									return yield* Effect.fail(
										new GitRepositoryConflict({ reason: "terminal_conflict" }),
									);
								}

								const event = yield* AppendEvent(transaction, {
									...(mutation.identity.agent_id === undefined
										? {}
										: { agent_id: mutation.identity.agent_id }),
									causation_id: mutation.identity.mutation_id,
									correlation_id: mutation.identity.request_message_id,
									occurred_at: updated_at,
									payload_at: (journal_sequence) =>
										DecodeMutationRow(
											{ ...updated, journal_sequence },
											true,
										).pipe(
											Effect.flatMap(({ projection }) =>
												Schema.decodeUnknownEffect(
													GitMutationUpdatedEvent,
													{
														onExcessProperty: "error",
													},
												)({
													mutation: projection,
													type: "git.mutation.updated",
												}),
											),
											Effect.mapError(() =>
												invariant("Recovered Git event is invalid"),
											),
										),
									...(mutation.identity.raw_origin === undefined
										? {}
										: { raw_origin: mutation.identity.raw_origin }),
									...(mutation.identity.run_id === undefined
										? {}
										: { run_id: mutation.identity.run_id }),
									thread_id: mutation.identity.thread_id,
								});
								yield* transaction
									.update(GitMutationOperations)
									.set({ journal_sequence: event.journal_sequence })
									.where(
										eq(
											GitMutationOperations.mutation_id,
											mutation.row.mutation_id,
										),
									);

								return (yield* ReadMutationTransaction(
									transaction,
									mutation.row.mutation_id,
								)).projection;
							}),
						);
						const approved_rows = yield* transaction
							.select()
							.from(GitMutationOperations)
							.where(eq(GitMutationOperations.lifecycle, "approved"))
							.orderBy(
								asc(GitMutationOperations.requested_at),
								asc(GitMutationOperations.mutation_id),
							);
						const approved = yield* Effect.forEach(approved_rows, (row) =>
							DecodeMutationRow(row).pipe(Effect.map(({ projection }) => projection)),
						);

						return { ambiguous, approved };
					}),
				),
			).pipe(
				Effect.mapError(normalize_error),
				Effect.tap((result) => {
					const journal_sequence = result.ambiguous.at(-1)?.journal_sequence;

					return journal_sequence === undefined
						? Effect.void
						: notifier.Publish(journal_sequence);
				}),
			);

		return {
			ClaimApproved,
			CommitSucceeded,
			CommitTerminal,
			ListPending,
			ReadMutation,
			ReadWorkspace,
			RecordWorkspace,
			RecoverDispatching,
			RequestMutation,
			ResolveMutation,
		};
	}),
);
