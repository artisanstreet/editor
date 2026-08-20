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
import { MakeTerminalCommits } from "./commits";
import { TerminalJournal, TerminalJournalLive } from "./journal";
import { MakeObservedTerminalStore } from "./observed-store";
import { TerminalQueries, TerminalQueriesLive } from "./queries";
import {
	DecodeStoredSession,
	DecodeStoredSnapshot,
	NormalizeTerminalError,
	RequireTerminalRow,
	TerminalCommandConflict,
	TerminalCommandStatus,
	TerminalCommandMatches,
	TerminalInvariantError,
	TerminalNotActive,
	TerminalNotFound,
	type TerminalCommand,
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
									stop_requested_generation: null,
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

							if (
								(current.state === "active" || current.state === "opening") &&
								current.stop_requested_generation !== current.generation
							) {
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
									stop_requested_generation: null,
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

							if (
								current.state !== "active" ||
								current.stop_requested_generation === current.generation
							) {
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

		/** The durable half of the lifecycle, once a claim has settled admission. */
		const commits = MakeTerminalCommits({ completion, database, journal, metadata, notifier });

		/** Observed shells share none of the claim machinery above; see the store. */
		const observed = MakeObservedTerminalStore({ database, journal, metadata, notifier });

		return {
			AdoptObserved: observed.AdoptObserved,
			Claim,
			CommitAmbiguous: commits.CommitAmbiguous,
			CommitCommand: commits.CommitCommand,
			CommitExit: commits.CommitExit,
			CommitRecovery: commits.CommitRecovery,
			CompleteCommand: commits.CompleteCommand,
			List: queries.List,
			ReadOwned: queries.ReadOwned,
			ReadStale: queries.ReadStale,
			RecoverStale: commits.RecoverStale,
		};
	}),
).pipe(
	Layer.provide(TerminalQueriesLive),
	Layer.provide(TerminalJournalLive),
	Layer.provide(TerminalCompletionLive),
);
