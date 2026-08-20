import { and, asc, eq, inArray, ne } from "drizzle-orm";
import { Context, Effect } from "effect";

import type { CommandEnvelope } from "@artisan/protocol";

import type { Database } from "../persistence/database";
import type { JournalNotifier } from "../persistence/journal-notifier";
import { OrchestrationRuns, TerminalCommands, TerminalSessions } from "../persistence/tables";
import type { RuntimeMetadata } from "../runtime/metadata";
import type { TerminalCompletion } from "./completion";
import { CommandEventInput, type TerminalJournal } from "./journal";
import {
	DecodeStoredSession,
	FailedSnapshot,
	NormalizeTerminalError,
	RequireTerminalRow,
	TerminalInvariantError,
	TerminalPersistenceFailure,
	type TerminalCommandClaim,
	type TerminalCommandTransition,
	type TerminalCommit,
	type TerminalLifecycleAction,
} from "./model";

/**
 * Writes the durable half of a terminal's lifecycle.
 *
 * Claiming a command and committing one are separate concerns that only share a
 * table: a claim decides whether the request may proceed at all — erasure,
 * idempotency, ownership, liveness — while a commit assumes that question is
 * settled and moves the row, journals the transition, and settles the claim in
 * one transaction. Splitting them keeps either side readable on its own.
 */
export interface TerminalCommitDependencies {
	readonly completion: Context.Service.Shape<typeof TerminalCompletion>;
	readonly database: Context.Service.Shape<typeof Database>;
	readonly journal: Context.Service.Shape<typeof TerminalJournal>;
	readonly metadata: Context.Service.Shape<typeof RuntimeMetadata>;
	readonly notifier: Context.Service.Shape<typeof JournalNotifier>;
}

export const MakeTerminalCommits = ({
	completion,
	database,
	journal,
	metadata,
	notifier,
}: TerminalCommitDependencies) => {
	const ReadClaim = (
		transaction: typeof database.client,
		message_id: string,
		generation: number,
	) =>
		transaction
			.select({
				generation: TerminalCommands.generation,
				status: TerminalCommands.status,
				terminal_id: TerminalCommands.terminal_id,
			})
			.from(TerminalCommands)
			.where(
				and(
					eq(TerminalCommands.message_id, message_id),
					eq(TerminalCommands.generation, generation),
				),
			)
			.limit(1)
			.pipe(
				Effect.flatMap(([claim]) =>
					claim?.status === "dispatching"
						? Effect.succeed(claim)
						: Effect.fail(
								new TerminalInvariantError({
									message: `Terminal command ${message_id} is not the expected dispatching generation`,
								}),
							),
				),
			);

	const ApplyTransition = (
		transaction: typeof database.client,
		row: typeof TerminalSessions.$inferSelect,
		transition: TerminalCommandTransition,
		adopt?: { readonly agent_id: string; readonly run_id: string },
		stop_requested = false,
	) =>
		Effect.gen(function* () {
			if (transition._tag === "current") {
				if (!adopt && !stop_requested) return yield* DecodeStoredSession(row);
				const [updated] = yield* transaction
					.update(TerminalSessions)
					.set({
						...(adopt
							? {
									owner_agent_id: adopt.agent_id,
									owner_kind: "agent" as const,
									owner_run_id: adopt.run_id,
									updated_at: yield* metadata.Now,
								}
							: {}),
						...(stop_requested ? { stop_requested_generation: row.generation } : {}),
					})
					.where(
						and(
							eq(TerminalSessions.terminal_id, row.terminal_id),
							eq(TerminalSessions.generation, row.generation),
						),
					)
					.returning();
				return yield* DecodeStoredSession(
					yield* RequireTerminalRow(
						updated,
						`Terminal ${row.terminal_id} adoption returned no row`,
					),
				);
			}

			if (
				transition._tag === "active" &&
				(!Number.isSafeInteger(transition.pid) || transition.pid <= 0)
			) {
				return yield* new TerminalPersistenceFailure({
					cause: new Error("Terminal driver returned an invalid process id"),
				});
			}

			const updated_at = yield* metadata.Now;
			const values = {
				...(adopt
					? {
							owner_agent_id: adopt.agent_id,
							owner_kind: "agent" as const,
							owner_run_id: adopt.run_id,
						}
					: {}),
				...(transition._tag === "active"
					? { pid: transition.pid, state: "active", updated_at }
					: transition._tag === "resize"
						? {
								cols: transition.cols,
								rows: transition.rows,
								updated_at,
							}
						: transition._tag === "pin"
							? { pinned: transition.pinned, updated_at }
							: {
									closed_at: updated_at,
									failure: transition.failure,
									pid: null,
									state: "failed",
									updated_at,
								}),
			};
			const [updated] = yield* transaction
				.update(TerminalSessions)
				.set(values)
				.where(
					and(
						eq(TerminalSessions.terminal_id, row.terminal_id),
						eq(TerminalSessions.generation, row.generation),
					),
				)
				.returning();

			if (!updated) {
				return yield* new TerminalInvariantError({
					message: `Terminal ${row.terminal_id} transition lost its generation`,
				});
			}

			return yield* DecodeStoredSession(updated);
		});

	const CommitCommand = (
		command: CommandEnvelope,
		generation: number,
		action: TerminalLifecycleAction,
		transition: TerminalCommandTransition,
	) =>
		database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const agent_ownership =
						command.agent_id !== undefined && command.run_id !== undefined
							? (yield* transaction
									.select({ agent_id: OrchestrationRuns.agent_id })
									.from(OrchestrationRuns)
									.where(
										and(
											eq(OrchestrationRuns.run_id, command.run_id),
											eq(OrchestrationRuns.thread_id, command.thread_id),
											eq(OrchestrationRuns.agent_id, command.agent_id),
											inArray(OrchestrationRuns.status, [
												"running",
												"waiting",
											]),
										),
									)
									.limit(1)).at(0)
							: undefined;
					const claim = yield* ReadClaim(transaction, command.message_id, generation);
					const [row] = yield* transaction
						.select()
						.from(TerminalSessions)
						.where(
							and(
								eq(TerminalSessions.terminal_id, claim.terminal_id),
								eq(TerminalSessions.thread_id, command.thread_id),
								eq(TerminalSessions.generation, generation),
							),
						)
						.limit(1);

					if (!row) {
						return yield* new TerminalInvariantError({
							message: `Terminal command ${command.message_id} projection generation is missing`,
						});
					}

					const stored = yield* ApplyTransition(
						transaction,
						row,
						transition,
						agent_ownership === undefined || command.run_id === undefined
							? undefined
							: { agent_id: agent_ownership.agent_id, run_id: command.run_id },
						action === "killed" || action === "closed",
					);
					const event = yield* journal.Append(
						transaction,
						CommandEventInput(command, action, stored.terminal),
					);

					yield* completion.CompleteClaim(transaction, {
						...(transition._tag === "failed" ? { failure: transition.failure } : {}),
						generation,
						journal_sequence: event.journal_sequence,
						message_id: command.message_id,
						status: transition._tag === "failed" ? "failed" : "completed",
					});

					return { event, stored } satisfies TerminalCommit;
				}),
			)
			.pipe(
				Effect.mapError(NormalizeTerminalError),
				Effect.tap((commit) => notifier.Publish(commit.event.journal_sequence)),
			);

	const CommitAmbiguous = (
		command: CommandEnvelope,
		claim: TerminalCommandClaim,
		failure: string,
	) =>
		database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					yield* ReadClaim(transaction, command.message_id, claim.generation);

					const [current] = yield* transaction
						.select()
						.from(TerminalSessions)
						.where(
							and(
								eq(TerminalSessions.terminal_id, claim.stored.terminal.terminal_id),
								eq(TerminalSessions.thread_id, command.thread_id),
							),
						)
						.limit(1);
					const updated_at = yield* metadata.Now;
					const stored =
						current?.generation === claim.generation
							? yield* ApplyTransition(transaction, current, {
									_tag: "failed",
									failure,
								})
							: FailedSnapshot(claim.stored, failure, updated_at);
					const event = yield* journal.Append(
						transaction,
						CommandEventInput(command, "failed", stored.terminal),
					);

					yield* completion.CompleteClaim(transaction, {
						failure,
						generation: claim.generation,
						journal_sequence: event.journal_sequence,
						message_id: command.message_id,
						status: "failed",
					});

					return { event, stored } satisfies TerminalCommit;
				}),
			)
			.pipe(
				Effect.mapError(NormalizeTerminalError),
				Effect.tap((commit) => notifier.Publish(commit.event.journal_sequence)),
			);

	const CommitExit = (
		terminal_id: string,
		generation: number,
		exit: {
			readonly exit_code: number | null;
			readonly reason: "closed" | "exited" | "killed" | "output_overflow";
			readonly signal: number | null;
		},
		action: TerminalLifecycleAction,
	) =>
		database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const updated_at = yield* metadata.Now;
					const [row] = yield* transaction
						.update(TerminalSessions)
						.set({
							closed_at: updated_at,
							exit_code: exit.exit_code,
							exit_reason: exit.reason,
							exit_signal: exit.signal,
							failure:
								exit.reason === "output_overflow"
									? "Terminal output exceeded the bounded stream capacity."
									: null,
							pid: null,
							state: exit.reason === "output_overflow" ? "failed" : "closed",
							updated_at,
						})
						.where(
							and(
								eq(TerminalSessions.terminal_id, terminal_id),
								eq(TerminalSessions.generation, generation),
							),
						)
						.returning();

					if (!row) {
						return yield* new TerminalInvariantError({
							message: `Terminal ${terminal_id} exit lost generation ${generation}`,
						});
					}

					const stored = yield* DecodeStoredSession(row);
					const correlation_id = yield* metadata.MakeId("message");
					const event = yield* journal.Append(transaction, {
						action,
						causation_id: correlation_id,
						correlation_id,
						terminal: stored.terminal,
					});

					return { event, stored } satisfies TerminalCommit;
				}),
			)
			.pipe(
				Effect.mapError(NormalizeTerminalError),
				Effect.tap((commit) => notifier.Publish(commit.event.journal_sequence)),
			);

	const CommitRecovery = (terminal_id: string, generation: number, failure: string) =>
		database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const [current] = yield* transaction
						.select()
						.from(TerminalSessions)
						.where(
							and(
								eq(TerminalSessions.terminal_id, terminal_id),
								eq(TerminalSessions.generation, generation),
							),
						)
						.limit(1);

					if (!current) {
						return yield* new TerminalInvariantError({
							message: `Terminal ${terminal_id} recovery lost generation ${generation}`,
						});
					}

					const stored = yield* ApplyTransition(transaction, current, {
						_tag: "failed",
						failure,
					});
					const correlation_id = yield* metadata.MakeId("message");
					const event = yield* journal.Append(transaction, {
						action: "recovered",
						causation_id: correlation_id,
						correlation_id,
						terminal: stored.terminal,
					});

					return { event, stored } satisfies TerminalCommit;
				}),
			)
			.pipe(
				Effect.mapError(NormalizeTerminalError),
				Effect.tap((commit) => notifier.Publish(commit.event.journal_sequence)),
			);

	const RecoverStale = (instance_id: string, failure: string) =>
		database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const stale = yield* transaction
						.select()
						.from(TerminalSessions)
						.where(
							and(
								inArray(TerminalSessions.state, ["opening", "active"]),
								ne(TerminalSessions.owner_instance_id, instance_id),
							),
						)
						.orderBy(
							asc(TerminalSessions.created_at),
							asc(TerminalSessions.terminal_id),
						);

					let watermark: number | undefined;
					for (const current of stale) {
						const stored = yield* ApplyTransition(transaction, current, {
							_tag: "failed",
							failure,
						});
						const correlation_id = yield* metadata.MakeId("message");
						const event = yield* journal.Append(transaction, {
							action: "recovered",
							causation_id: correlation_id,
							correlation_id,
							terminal: stored.terminal,
						});
						watermark = event.journal_sequence;
					}

					return { count: stale.length, watermark };
				}),
			)
			.pipe(
				Effect.mapError(NormalizeTerminalError),
				Effect.tap(({ watermark }) =>
					watermark === undefined ? Effect.void : notifier.Publish(watermark),
				),
				Effect.map(({ count }) => count),
			);

	const CompleteCommand = (
		message_id: string,
		generation: number,
		status: "completed" | "failed",
		journal_sequence: number,
		failure?: string,
	) =>
		completion.CompleteCommand({
			...(failure ? { failure } : {}),
			generation,
			journal_sequence,
			message_id,
			status,
		});

	return {
		CommitAmbiguous,
		CommitCommand,
		CommitExit,
		CommitRecovery,
		CompleteCommand,
		RecoverStale,
	};
};
