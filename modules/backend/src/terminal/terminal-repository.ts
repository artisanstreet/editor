import { and, asc, eq, inArray, ne } from "drizzle-orm";
import { Context, Data, Effect, Layer, Schema } from "effect";

import {
	EventEnvelope,
	TerminalSession,
	type CommandEnvelope,
	type TerminalLifecycleEvent,
} from "@artisan/protocol";

import { Database } from "../persistence/database";
import { JournalNotifier } from "../persistence/journal-notifier";
import {
	EventStreams,
	JournalCommands,
	JournalEvents,
	TerminalCommands,
	TerminalSessions,
	ThreadErasureClaims,
	Threads,
} from "../persistence/schema";
import { RuntimeMetadata } from "../runtime/runtime-metadata";
import { RecordThreadActivity } from "../threads/internal/thread-activity";

export type TerminalCommand = Extract<
	CommandEnvelope["payload"],
	{ readonly type: `terminal.${string}` }
>;

export type TerminalLifecycleAction = TerminalLifecycleEvent["action"];

export interface StoredTerminalSession {
	readonly env?: Readonly<Record<string, string>> | undefined;
	readonly terminal: TerminalSession;
}

export interface TerminalCommandClaim {
	readonly command_status: "completed" | "dispatching" | "failed";
	readonly generation: number;
	readonly status: "accepted" | "duplicate";
	readonly stored: StoredTerminalSession;
}

export type TerminalCommandTransition =
	| { readonly _tag: "active"; readonly pid: number }
	| { readonly _tag: "current" }
	| { readonly _tag: "failed"; readonly failure: string }
	| { readonly _tag: "resize"; readonly cols: number; readonly rows: number };

export interface TerminalCommit {
	readonly event: EventEnvelope;
	readonly stored: StoredTerminalSession;
}

/** Reports a reused command identifier carrying different intent. */
export class TerminalCommandConflict extends Data.TaggedError("TerminalCommandConflict")<{
	readonly message_id: string;
}> {}

/** Hides whether a terminal is absent or owned by another thread. */
export class TerminalNotFound extends Data.TaggedError("TerminalNotFound")<{
	readonly terminal_id: string;
}> {}

/** Reports an operation which requires a currently active terminal. */
export class TerminalNotActive extends Data.TaggedError("TerminalNotActive")<{
	readonly terminal_id: string;
}> {}

/** Reports malformed terminal state read from durable storage. */
export class TerminalInvariantError extends Data.TaggedError("TerminalInvariantError")<{
	readonly message: string;
}> {}

/** Wraps an unexpected terminal persistence failure. */
export class TerminalPersistenceFailure extends Data.TaggedError("TerminalPersistenceFailure")<{
	readonly cause: unknown;
}> {}

export type TerminalRepositoryError =
	| TerminalCommandConflict
	| TerminalInvariantError
	| TerminalNotActive
	| TerminalNotFound
	| TerminalPersistenceFailure;

/** Persists generation-bound terminal claims and atomic lifecycle commits. */
export class TerminalRepository extends Context.Service<
	TerminalRepository,
	{
		readonly Claim: (
			command: CommandEnvelope,
			instance_id: string,
		) => Effect.Effect<TerminalCommandClaim, TerminalRepositoryError>;
		readonly CommitAmbiguous: (
			command: CommandEnvelope,
			claim: TerminalCommandClaim,
			failure: string,
		) => Effect.Effect<TerminalCommit, TerminalRepositoryError>;
		readonly CommitCommand: (
			command: CommandEnvelope,
			generation: number,
			action: TerminalLifecycleAction,
			transition: TerminalCommandTransition,
		) => Effect.Effect<TerminalCommit, TerminalRepositoryError>;
		readonly CommitExit: (
			terminal_id: string,
			generation: number,
			exit: {
				readonly exit_code: number | null;
				readonly reason: "closed" | "exited" | "killed" | "output_overflow";
				readonly signal: number | null;
			},
			action: TerminalLifecycleAction,
		) => Effect.Effect<TerminalCommit, TerminalRepositoryError>;
		readonly CommitRecovery: (
			terminal_id: string,
			generation: number,
			failure: string,
		) => Effect.Effect<TerminalCommit, TerminalRepositoryError>;
		readonly CompleteCommand: (
			message_id: string,
			generation: number,
			status: "completed" | "failed",
			journal_sequence: number,
			failure?: string,
		) => Effect.Effect<void, TerminalRepositoryError>;
		readonly List: (
			thread_id: string,
			workspace_id: string,
		) => Effect.Effect<ReadonlyArray<TerminalSession>, TerminalRepositoryError>;
		readonly ReadOwned: (
			terminal_id: string,
			thread_id: string,
		) => Effect.Effect<StoredTerminalSession, TerminalRepositoryError>;
		readonly ReadStale: (
			instance_id: string,
		) => Effect.Effect<ReadonlyArray<StoredTerminalSession>, TerminalRepositoryError>;
	}
>()("Artisan/TerminalRepository") {}

const StringArray = Schema.Array(Schema.String);
const EnvironmentRecord = Schema.Record(Schema.String, Schema.String);
const TerminalCommandStatus = Schema.Literals(["completed", "dispatching", "failed"]);
const StoredTerminalSnapshot = Schema.Struct({
	env: Schema.optional(EnvironmentRecord),
	terminal: TerminalSession,
});

function normalize_error(error: unknown): TerminalRepositoryError {
	if (
		error instanceof TerminalCommandConflict ||
		error instanceof TerminalInvariantError ||
		error instanceof TerminalNotActive ||
		error instanceof TerminalNotFound
	) {
		return error;
	}

	return new TerminalPersistenceFailure({ cause: error });
}

const ParseJson = (json: string, context: string) =>
	Effect.try({
		try: () => JSON.parse(json) as unknown,
		catch: () => new TerminalInvariantError({ message: `${context} contains invalid JSON` }),
	});

const DecodeStoredSnapshot = (json: string, context: string) =>
	ParseJson(json, context).pipe(
		Effect.flatMap((value) =>
			Schema.decodeUnknownEffect(StoredTerminalSnapshot, {
				onExcessProperty: "error",
			})(value).pipe(
				Effect.mapError(
					() =>
						new TerminalInvariantError({
							message: `${context} does not match the terminal snapshot schema`,
						}),
				),
			),
		),
	);

const DecodeStoredSession = (row: typeof TerminalSessions.$inferSelect) =>
	Effect.gen(function* () {
		const args_value = yield* ParseJson(row.args_json, `Terminal ${row.terminal_id} args`);
		const args = yield* Schema.decodeUnknownEffect(StringArray)(args_value).pipe(
			Effect.mapError(
				() =>
					new TerminalInvariantError({
						message: `Terminal ${row.terminal_id} args do not match the schema`,
					}),
			),
		);
		const env = row.env_json
			? yield* ParseJson(row.env_json, `Terminal ${row.terminal_id} environment`).pipe(
					Effect.flatMap((value) =>
						Schema.decodeUnknownEffect(EnvironmentRecord)(value).pipe(
							Effect.mapError(
								() =>
									new TerminalInvariantError({
										message: `Terminal ${row.terminal_id} environment does not match the schema`,
									}),
							),
						),
					),
				)
			: undefined;
		const terminal = yield* Schema.decodeUnknownEffect(TerminalSession, {
			onExcessProperty: "error",
		})({
			args,
			closed_at: row.closed_at ?? undefined,
			cols: row.cols,
			created_at: row.created_at,
			executable: row.executable,
			exit_code: row.exit_code ?? undefined,
			exit_reason: row.exit_reason ?? undefined,
			exit_signal: row.exit_signal ?? undefined,
			failure: row.failure ?? undefined,
			generation: row.generation,
			pid: row.pid ?? undefined,
			rows: row.rows,
			state: row.state,
			terminal_id: row.terminal_id,
			thread_id: row.thread_id,
			updated_at: row.updated_at,
			workspace_id: row.workspace_id,
			working_directory: row.working_directory,
		}).pipe(
			Effect.mapError(
				() =>
					new TerminalInvariantError({
						message: `Terminal ${row.terminal_id} does not match the protocol schema`,
					}),
			),
		);

		return { ...(env ? { env } : {}), terminal } satisfies StoredTerminalSession;
	});

function command_matches(
	command: CommandEnvelope,
	existing: {
		readonly agent_id: string | null;
		readonly causation_id: string | null;
		readonly origin: string;
		readonly payload_json: string;
		readonly raw_origin_json: string | null;
		readonly run_id: string | null;
		readonly schema_version: number;
		readonly sent_at: string;
		readonly thread_id: string;
	},
) {
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

function failed_snapshot(
	stored: StoredTerminalSession,
	failure: string,
	updated_at: string,
): StoredTerminalSession {
	const { pid: _pid, ...without_pid } = stored.terminal;

	return {
		...(stored.env ? { env: stored.env } : {}),
		terminal: {
			...without_pid,
			closed_at: updated_at,
			failure,
			state: "failed",
			updated_at,
		},
	};
}

export const TerminalRepositoryLive = Layer.effect(
	TerminalRepository,
	Effect.gen(function* () {
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;
		const notifier = yield* JournalNotifier;

		const AppendEvent = (
			transaction: typeof database.client,
			input: {
				readonly action: TerminalLifecycleAction;
				readonly agent_id?: string;
				readonly causation_id: string;
				readonly correlation_id: string;
				readonly raw_origin?: {
					readonly provider: string;
					readonly reference: string;
				};
				readonly run_id?: string;
				readonly terminal: TerminalSession;
			},
		) =>
			Effect.gen(function* () {
				const stream_id = `thread:${input.terminal.thread_id}`;
				const [stream] = yield* transaction
					.select({ last_sequence: EventStreams.last_sequence })
					.from(EventStreams)
					.where(eq(EventStreams.stream_id, stream_id))
					.limit(1);
				const sequence = (stream?.last_sequence ?? 0) + 1;
				const event_id = yield* metadata.MakeId("event");
				const occurred_at = yield* metadata.Now;
				const payload = {
					action: input.action,
					terminal: input.terminal,
					type: "terminal.lifecycle" as const,
				} satisfies TerminalLifecycleEvent;

				yield* RecordThreadActivity(
					transaction,
					input.terminal.thread_id,
					occurred_at,
					payload,
				);

				if (stream) {
					yield* transaction
						.update(EventStreams)
						.set({ last_sequence: sequence })
						.where(eq(EventStreams.stream_id, stream_id));
				} else {
					yield* transaction.insert(EventStreams).values({
						last_sequence: sequence,
						stream_id,
					});
				}

				const [inserted] = yield* transaction
					.insert(JournalEvents)
					.values({
						agent_id: input.agent_id ?? null,
						causation_id: input.causation_id,
						correlation_id: input.correlation_id,
						event_id,
						event_type: payload.type,
						occurred_at,
						origin: "backend",
						payload_json: JSON.stringify(payload),
						raw_origin_json: input.raw_origin ? JSON.stringify(input.raw_origin) : null,
						run_id: input.run_id ?? null,
						schema_version: 1,
						stream_id,
						stream_sequence: sequence,
						thread_id: input.terminal.thread_id,
					})
					.returning({ journal_sequence: JournalEvents.sequence });

				return {
					...(input.agent_id ? { agent_id: input.agent_id } : {}),
					causation_id: input.causation_id,
					correlation_id: input.correlation_id,
					journal_sequence: inserted!.journal_sequence,
					kind: "event" as const,
					message_id: event_id,
					origin: "backend" as const,
					payload,
					protocol_version: 1 as const,
					...(input.raw_origin ? { raw_origin: input.raw_origin } : {}),
					...(input.run_id ? { run_id: input.run_id } : {}),
					schema_version: 1 as const,
					sequence,
					sent_at: occurred_at,
					stream_id,
					thread_id: input.terminal.thread_id,
				} satisfies EventEnvelope;
			});

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

		const CompleteClaim = (
			transaction: typeof database.client,
			input: {
				readonly failure?: string;
				readonly generation: number;
				readonly journal_sequence: number;
				readonly message_id: string;
				readonly status: "completed" | "failed";
			},
		) =>
			Effect.gen(function* () {
				const updated_at = yield* metadata.Now;
				const updated = yield* transaction
					.update(TerminalCommands)
					.set({
						failure: input.failure ?? null,
						journal_sequence: input.journal_sequence,
						status: input.status,
						updated_at,
					})
					.where(
						and(
							eq(TerminalCommands.message_id, input.message_id),
							eq(TerminalCommands.generation, input.generation),
							eq(TerminalCommands.status, "dispatching"),
						),
					)
					.returning({ message_id: TerminalCommands.message_id });

				if (updated.length !== 1) {
					return yield* new TerminalInvariantError({
						message: `Terminal command ${input.message_id} completion did not update its dispatching claim`,
					});
				}
			});

		const CommandEventInput = (
			command: CommandEnvelope,
			action: TerminalLifecycleAction,
			terminal: TerminalSession,
		) => ({
			...(command.agent_id ? { agent_id: command.agent_id } : {}),
			action,
			causation_id: command.message_id,
			correlation_id: command.message_id,
			...(command.raw_origin ? { raw_origin: command.raw_origin } : {}),
			...(command.run_id ? { run_id: command.run_id } : {}),
			terminal,
		});

		const ReadOwned = (terminal_id: string, thread_id: string) =>
			database.client
				.select()
				.from(TerminalSessions)
				.where(
					and(
						eq(TerminalSessions.terminal_id, terminal_id),
						eq(TerminalSessions.thread_id, thread_id),
					),
				)
				.limit(1)
				.pipe(
					Effect.flatMap(
						([row]): Effect.Effect<
							StoredTerminalSession,
							TerminalInvariantError | TerminalNotFound
						> => {
							if (!row) {
								return Effect.fail(new TerminalNotFound({ terminal_id }));
							}

							return DecodeStoredSession(row);
						},
					),
					Effect.mapError(normalize_error),
				);

		const Claim = (command: CommandEnvelope, instance_id: string) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
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
							if (!command_matches(command, existing_command)) {
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
									owner_instance_id: instance_id,
									rows: payload.rows,
									state: "opening",
									terminal_id: payload.terminal_id,
									thread_id: command.thread_id,
									updated_at: accepted_at,
									working_directory: payload.working_directory,
									workspace_id: payload.workspace_id,
								})
								.returning();

							row = inserted!;
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

							row = restarted!;
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
				.pipe(Effect.mapError(normalize_error));

		const ApplyTransition = (
			transaction: typeof database.client,
			row: typeof TerminalSessions.$inferSelect,
			transition: TerminalCommandTransition,
		) =>
			Effect.gen(function* () {
				if (transition._tag === "current") {
					return yield* DecodeStoredSession(row);
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
				const values =
					transition._tag === "active"
						? { pid: transition.pid, state: "active", updated_at }
						: transition._tag === "resize"
							? {
									cols: transition.cols,
									rows: transition.rows,
									updated_at,
								}
							: {
									closed_at: updated_at,
									failure: transition.failure,
									pid: null,
									state: "failed",
									updated_at,
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

						const stored = yield* ApplyTransition(transaction, row, transition);
						const event = yield* AppendEvent(
							transaction,
							CommandEventInput(command, action, stored.terminal),
						);

						yield* CompleteClaim(transaction, {
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
					Effect.mapError(normalize_error),
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
								: failed_snapshot(claim.stored, failure, updated_at);
						const event = yield* AppendEvent(
							transaction,
							CommandEventInput(command, "failed", stored.terminal),
						);

						yield* CompleteClaim(transaction, {
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
					Effect.mapError(normalize_error),
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
						const event = yield* AppendEvent(transaction, {
							action,
							causation_id: correlation_id,
							correlation_id,
							terminal: stored.terminal,
						});

						return { event, stored } satisfies TerminalCommit;
					}),
				)
				.pipe(
					Effect.mapError(normalize_error),
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
						const event = yield* AppendEvent(transaction, {
							action: "recovered",
							causation_id: correlation_id,
							correlation_id,
							terminal: stored.terminal,
						});

						return { event, stored } satisfies TerminalCommit;
					}),
				)
				.pipe(
					Effect.mapError(normalize_error),
					Effect.tap((commit) => notifier.Publish(commit.event.journal_sequence)),
				);

		const CompleteCommand = (
			message_id: string,
			generation: number,
			status: "completed" | "failed",
			journal_sequence: number,
			failure?: string,
		) =>
			database.client
				.transaction((transaction) =>
					CompleteClaim(transaction, {
						...(failure ? { failure } : {}),
						generation,
						journal_sequence,
						message_id,
						status,
					}),
				)
				.pipe(Effect.mapError(normalize_error));

		const ReadStale = (instance_id: string) =>
			database.client
				.select()
				.from(TerminalSessions)
				.where(
					and(
						inArray(TerminalSessions.state, ["opening", "active"]),
						ne(TerminalSessions.owner_instance_id, instance_id),
					),
				)
				.orderBy(asc(TerminalSessions.created_at), asc(TerminalSessions.terminal_id))
				.pipe(
					Effect.flatMap((rows) => Effect.forEach(rows, DecodeStoredSession)),
					Effect.mapError(normalize_error),
				);

		const List = (thread_id: string, workspace_id: string) =>
			database.client
				.select()
				.from(TerminalSessions)
				.where(
					and(
						eq(TerminalSessions.thread_id, thread_id),
						eq(TerminalSessions.workspace_id, workspace_id),
					),
				)
				.orderBy(asc(TerminalSessions.created_at), asc(TerminalSessions.terminal_id))
				.pipe(
					Effect.flatMap((rows) => Effect.forEach(rows, DecodeStoredSession)),
					Effect.map((stored) => stored.map(({ terminal }) => terminal)),
					Effect.mapError(normalize_error),
				);

		return {
			Claim,
			CommitAmbiguous,
			CommitCommand,
			CommitExit,
			CommitRecovery,
			CompleteCommand,
			List,
			ReadOwned,
			ReadStale,
		};
	}),
);
