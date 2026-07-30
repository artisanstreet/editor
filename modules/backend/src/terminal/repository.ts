import { and, eq, inArray } from "drizzle-orm";
import { Effect, Layer, Schema } from "effect";

import type { CommandEnvelope } from "@artisan/protocol";

import { Database } from "../persistence/database";
import { JournalNotifier } from "../persistence/journal-notifier";
import {
	JournalCommands,
	OrchestrationRuns,
	TerminalCommands,
	TerminalSessions,
	ThreadErasureClaims,
	Threads,
} from "../persistence/tables";
import { RuntimeMetadata } from "../runtime/metadata";
import { TerminalRepository } from "./contract";
import { TerminalCompletion, TerminalCompletionLive } from "./completion";
import { CommandEventInput, TerminalJournal, TerminalJournalLive } from "./journal";
import { TerminalQueries, TerminalQueriesLive } from "./queries";
import {
	DecodeStoredSession,
	DecodeStoredSnapshot,
	FailedSnapshot,
	NormalizeTerminalError,
	RequireTerminalRow,
	TerminalCommandConflict,
	TerminalCommandStatus,
	TerminalCommandMatches,
	TerminalInvariantError,
	TerminalNotActive,
	TerminalNotFound,
	TerminalPersistenceFailure,
	type TerminalCommand,
	type TerminalCommandClaim,
	type TerminalCommandTransition,
	type TerminalCommit,
	type TerminalLifecycleAction,
} from "./model";

export const TerminalRepositoryLive = Layer.effect(
	TerminalRepository,
	Effect.gen(function* () {
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;
		const notifier = yield* JournalNotifier;
		const queries = yield* TerminalQueries;
		const journal = yield* TerminalJournal;
		const completion = yield* TerminalCompletion;

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

		const Claim = (command: CommandEnvelope, instance_id: string) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const ResolveAgentOwnership = () =>
							command.agent_id === undefined || command.run_id === undefined
								? Effect.succeed(undefined)
								: transaction
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
										.limit(1)
										.pipe(
											Effect.map(([run]) =>
												run === undefined
													? undefined
													: {
															agent_id: run.agent_id,
															run_id: command.run_id,
														},
											),
										);
						const agent_ownership = yield* ResolveAgentOwnership();
						const payload = command.payload as TerminalCommand;
						const [erasure_claim] = yield* transaction
							.select({ thread_id: ThreadErasureClaims.thread_id })
							.from(ThreadErasureClaims)
							.where(eq(ThreadErasureClaims.thread_id, command.thread_id))
							.limit(1);

						if (erasure_claim) {
							return yield* new TerminalNotFound({
								terminal_id: payload.terminal_id,
							});
						}

						const [existing_command] = yield* transaction
							.select({
								agent_id: JournalCommands.agent_id,
								causation_id: JournalCommands.causation_id,
								origin: JournalCommands.origin,
								payload_json: JournalCommands.payload_json,
								raw_origin_json: JournalCommands.raw_origin_json,
								run_id: JournalCommands.run_id,
								schema_version: JournalCommands.schema_version,
								sent_at: JournalCommands.sent_at,
								thread_id: JournalCommands.thread_id,
							})
							.from(JournalCommands)
							.where(eq(JournalCommands.message_id, command.message_id))
							.limit(1);

						if (existing_command) {
							if (!TerminalCommandMatches(command, existing_command)) {
								return yield* new TerminalCommandConflict({
									message_id: command.message_id,
								});
							}

							const [terminal_command] = yield* transaction
								.select()
								.from(TerminalCommands)
								.where(eq(TerminalCommands.message_id, command.message_id))
								.limit(1);

							if (!terminal_command) {
								return yield* new TerminalInvariantError({
									message: `Terminal command ${command.message_id} has no claim`,
								});
							}

							const command_status = yield* Schema.decodeUnknownEffect(
								TerminalCommandStatus,
							)(terminal_command.status).pipe(
								Effect.mapError(
									() =>
										new TerminalInvariantError({
											message: `Terminal command ${command.message_id} has an invalid status`,
										}),
								),
							);
							const stored = yield* DecodeStoredSnapshot(
								terminal_command.claimed_session_json,
								`Terminal command ${command.message_id} snapshot`,
							);

							if (
								terminal_command.generation !== stored.terminal.generation ||
								terminal_command.terminal_id !== stored.terminal.terminal_id ||
								stored.terminal.thread_id !== command.thread_id
							) {
								return yield* new TerminalInvariantError({
									message: `Terminal command ${command.message_id} snapshot does not match its claim`,
								});
							}

							return {
								command_status,
								generation: terminal_command.generation,
								status: "duplicate" as const,
								stored,
							};
						}

						const [thread] = yield* transaction
							.select({ thread_id: Threads.thread_id })
							.from(Threads)
							.where(eq(Threads.thread_id, command.thread_id))
							.limit(1);

						if (!thread) {
							return yield* new TerminalNotFound({
								terminal_id: payload.terminal_id,
							});
						}

						const [current] = yield* transaction
							.select()
							.from(TerminalSessions)
							.where(eq(TerminalSessions.terminal_id, payload.terminal_id))
							.limit(1);
						const accepted_at = yield* metadata.Now;
						let row: typeof TerminalSessions.$inferSelect;

						if (payload.type === "terminal.open") {
							if (current) {
								return yield* new TerminalCommandConflict({
									message_id: command.message_id,
								});
							}

							const [inserted] = yield* transaction
								.insert(TerminalSessions)
								.values({
									args_json: JSON.stringify(payload.args),
									cols: payload.cols,
									created_at: accepted_at,
									env_json: payload.env ? JSON.stringify(payload.env) : null,
									executable: payload.executable,
									generation: 1,
									owner_kind: agent_ownership === undefined ? "user" : "agent",
									owner_agent_id:
										agent_ownership === undefined
											? null
											: agent_ownership.agent_id,
									owner_run_id:
										agent_ownership === undefined
											? null
											: agent_ownership.run_id,
									owner_instance_id: instance_id,
									pinned: false,
									rows: payload.rows,
									state: "opening",
									terminal_id: payload.terminal_id,
									thread_id: command.thread_id,
									updated_at: accepted_at,
									working_directory: payload.working_directory,
									workspace_id: payload.workspace_id,
								})
								.returning();

							row = yield* RequireTerminalRow(
								inserted,
								`Terminal ${payload.terminal_id} insert returned no row`,
							);
						} else if (payload.type === "terminal.restart") {
							if (!current || current.thread_id !== command.thread_id) {
								return yield* new TerminalNotFound({
									terminal_id: payload.terminal_id,
								});
							}

							if (current.state === "active" || current.state === "opening") {
								return yield* new TerminalNotActive({
									terminal_id: payload.terminal_id,
								});
							}

							const [restarted] = yield* transaction
								.update(TerminalSessions)
								.set({
									closed_at: null,
									exit_code: null,
									exit_reason: null,
									exit_signal: null,
									failure: null,
									generation: current.generation + 1,
									owner_instance_id: instance_id,
									pid: null,
									state: "opening",
									updated_at: accepted_at,
								})
								.where(eq(TerminalSessions.terminal_id, payload.terminal_id))
								.returning();

							row = yield* RequireTerminalRow(
								restarted,
								`Terminal ${payload.terminal_id} restart returned no row`,
							);
						} else if (payload.type === "terminal.pin") {
							if (!current || current.thread_id !== command.thread_id) {
								return yield* new TerminalNotFound({
									terminal_id: payload.terminal_id,
								});
							}
							row = current;
						} else {
							if (!current || current.thread_id !== command.thread_id) {
								return yield* new TerminalNotFound({
									terminal_id: payload.terminal_id,
								});
							}

							if (current.state !== "active") {
								return yield* new TerminalNotActive({
									terminal_id: payload.terminal_id,
								});
							}

							row = current;
						}

						const stored = yield* DecodeStoredSession(row);

						yield* transaction.insert(JournalCommands).values({
							accepted_at,
							agent_id: command.agent_id ?? null,
							causation_id: command.causation_id ?? null,
							message_id: command.message_id,
							origin: command.origin,
							payload_json: JSON.stringify(command.payload),
							payload_type: command.payload.type,
							raw_origin_json: command.raw_origin
								? JSON.stringify(command.raw_origin)
								: null,
							run_id: command.run_id ?? null,
							schema_version: command.schema_version,
							sent_at: command.sent_at,
							status: "accepted",
							thread_id: command.thread_id,
						});
						yield* transaction.insert(TerminalCommands).values({
							claimed_session_json: JSON.stringify(stored),
							created_at: accepted_at,
							generation: stored.terminal.generation,
							message_id: command.message_id,
							payload_json: JSON.stringify(command.payload),
							status: "dispatching",
							terminal_id: payload.terminal_id,
							updated_at: accepted_at,
						});

						return {
							command_status: "dispatching" as const,
							generation: stored.terminal.generation,
							status: "accepted" as const,
							stored,
						};
					}),
				)
				.pipe(Effect.mapError(NormalizeTerminalError));

		const ApplyTransition = (
			transaction: typeof database.client,
			row: typeof TerminalSessions.$inferSelect,
			transition: TerminalCommandTransition,
			adopt?: { readonly agent_id: string; readonly run_id: string },
		) =>
			Effect.gen(function* () {
				if (transition._tag === "current") {
					if (!adopt) return yield* DecodeStoredSession(row);
					const [updated] = yield* transaction
						.update(TerminalSessions)
						.set({
							owner_agent_id: adopt.agent_id,
							owner_kind: "agent",
							owner_run_id: adopt.run_id,
							updated_at: yield* metadata.Now,
						})
						.where(eq(TerminalSessions.terminal_id, row.terminal_id))
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
						);
						const event = yield* journal.Append(
							transaction,
							CommandEventInput(command, action, stored.terminal),
						);

						yield* completion.CompleteClaim(transaction, {
							...(transition._tag === "failed"
								? { failure: transition.failure }
								: {}),
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
									eq(
										TerminalSessions.terminal_id,
										claim.stored.terminal.terminal_id,
									),
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
			Claim,
			CommitAmbiguous,
			CommitCommand,
			CommitExit,
			CommitRecovery,
			CompleteCommand,
			List: queries.List,
			ReadOwned: queries.ReadOwned,
			ReadStale: queries.ReadStale,
		};
	}),
).pipe(
	Layer.provide(TerminalQueriesLive),
	Layer.provide(TerminalJournalLive),
	Layer.provide(TerminalCompletionLive),
);
