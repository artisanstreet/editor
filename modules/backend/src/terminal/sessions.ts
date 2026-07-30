import {
	Cause,
	Context,
	Deferred,
	Effect,
	Exit,
	Layer,
	Ref,
	Scope,
	Semaphore,
	Stream,
} from "effect";

import type { CommandEnvelope, EventEnvelope, TerminalSession } from "@artisan/protocol";

import { JournalStore, type JournalStoreError } from "../persistence/journal-store";
import { RuntimeMetadata } from "../runtime/metadata";
import {
	TerminalDriver,
	TerminalDriverError,
	type TerminalDriverExit,
	type TerminalDriverHandle,
} from "./driver";
import { TerminalRepository } from "./contract";
import {
	TerminalInvariantError,
	TerminalNotActive,
	TerminalNotFound,
	type StoredTerminalSession,
	type TerminalCommand,
	type TerminalCommandClaim,
	type TerminalCommit,
	type TerminalLifecycleAction,
	type TerminalRepositoryError,
} from "./model";

interface LiveTerminal {
	readonly done: Deferred.Deferred<StoredTerminalSession, TerminalSessionError>;
	readonly generation: number;
	readonly handle: TerminalDriverHandle;
	readonly scope: Scope.Closeable;
	readonly thread_id: string;
	readonly workspace_id: string;
}

/** Unions terminal persistence and canonical journal failures. */
export type TerminalSessionError = JournalStoreError | TerminalRepositoryError;

/** Returns the durable outcome and canonical events for one terminal command. */
export interface TerminalCommandAcceptance {
	readonly events: ReadonlyArray<EventEnvelope>;
	readonly journal_sequence: number;
	readonly status: "accepted" | "duplicate";
	readonly terminal: TerminalSession;
}

/** Owns durable terminal metadata and live PTY handles for the backend runtime. */
export class TerminalSessionService extends Context.Service<
	TerminalSessionService,
	{
		readonly Handle: (
			command: CommandEnvelope,
		) => Effect.Effect<TerminalCommandAcceptance, TerminalSessionError>;
		readonly HandleAsAgent: (
			command: CommandEnvelope,
			authority: { readonly agent_id: string; readonly run_id: string },
		) => Effect.Effect<TerminalCommandAcceptance, TerminalSessionError>;
		readonly List: (
			thread_id: string,
			workspace_id: string,
		) => Effect.Effect<ReadonlyArray<TerminalSession>, TerminalSessionError>;
		readonly Output: (input: {
			readonly terminal_id: string;
			readonly thread_id: string;
			readonly workspace_id: string;
		}) => Effect.Effect<Stream.Stream<Uint8Array>, TerminalSessionError>;
		readonly ReadOutput: (
			terminal_id: string,
		) => Effect.Effect<Stream.Stream<Uint8Array>, TerminalSessionError>;
		readonly QuiesceThread: (thread_id: string) => Effect.Effect<void, TerminalSessionError>;
	}
>()("Artisan/TerminalSessionService") {}

function failure_message(cause: Cause.Cause<unknown>) {
	const failure = Cause.squash(cause);
	const nested =
		typeof failure === "object" &&
		failure !== null &&
		"cause" in failure &&
		failure.cause instanceof Error
			? failure.cause.message
			: undefined;
	const message = failure instanceof Error ? failure.message : String(failure);

	return nested || message || "The terminal driver failed without an error message.";
}

function exit_action(reason: TerminalDriverExit["reason"]): TerminalLifecycleAction {
	if (reason === "output_overflow") {
		return "failed";
	}

	return reason === "exited" ? "exited" : reason;
}

function acceptance(
	commit: TerminalCommit,
	status: "accepted" | "duplicate",
): TerminalCommandAcceptance {
	return {
		events: [commit.event],
		journal_sequence: commit.event.journal_sequence,
		status,
		terminal: commit.stored.terminal,
	};
}

export const TerminalSessionServiceLive = Layer.effect(
	TerminalSessionService,
	Effect.gen(function* () {
		const driver = yield* TerminalDriver;
		const journal = yield* JournalStore;
		const metadata = yield* RuntimeMetadata;
		const repository = yield* TerminalRepository;
		const service_scope = yield* Scope.make();
		const live_terminals = yield* Ref.make(new Map<string, LiveTerminal>());
		const quiesced_threads = yield* Ref.make(new Set<string>());
		const command_lock = yield* Semaphore.make(1);

		const RemoveLive = (terminal_id: string, generation: number) =>
			Ref.update(live_terminals, (terminals) => {
				const current = terminals.get(terminal_id);

				if (!current || current.generation !== generation) {
					return terminals;
				}

				const next = new Map(terminals);

				next.delete(terminal_id);

				return next;
			});

		const ObserveExit = (terminal_id: string, live: LiveTerminal) =>
			Effect.gen(function* () {
				const exit = yield* live.handle.Exit;
				const commit = yield* repository.CommitExit(
					terminal_id,
					live.generation,
					exit,
					exit_action(exit.reason),
				);

				yield* Deferred.succeed(live.done, commit.stored);
			}).pipe(
				Effect.catch((error) => Deferred.fail(live.done, error)),
				Effect.ensuring(
					Effect.gen(function* () {
						yield* RemoveLive(terminal_id, live.generation);
						yield* Scope.close(live.scope, Exit.void);
					}),
				),
			);

		const RegisterLive = (
			stored: StoredTerminalSession,
			handle: TerminalDriverHandle,
			scope: Scope.Closeable,
		) =>
			Effect.gen(function* () {
				const done = yield* Deferred.make<StoredTerminalSession, TerminalSessionError>();
				const live = {
					done,
					generation: stored.terminal.generation,
					handle,
					scope,
					thread_id: stored.terminal.thread_id,
					workspace_id: stored.terminal.workspace_id,
				} satisfies LiveTerminal;

				yield* Ref.update(live_terminals, (terminals) =>
					new Map(terminals).set(stored.terminal.terminal_id, live),
				);
				yield* Effect.forkIn(ObserveExit(stored.terminal.terminal_id, live), service_scope);

				return live;
			});

		const ReplayExisting = (
			command: CommandEnvelope,
			claim: TerminalCommandClaim,
			events: ReadonlyArray<EventEnvelope>,
			finalize: boolean,
		) =>
			Effect.gen(function* () {
				const event = events.at(-1);

				if (!event || event.payload.type !== "terminal.lifecycle") {
					return yield* new TerminalInvariantError({
						message: `Terminal command ${command.message_id} has no terminal lifecycle event`,
					});
				}

				if (finalize) {
					yield* repository.CompleteCommand(
						command.message_id,
						claim.generation,
						event.payload.action === "failed" ? "failed" : "completed",
						event.journal_sequence,
						event.payload.terminal.failure,
					);
				}

				return {
					events,
					journal_sequence: event.journal_sequence,
					status: "duplicate" as const,
					terminal: event.payload.terminal,
				};
			});

		const Start = (
			command: CommandEnvelope,
			claim: TerminalCommandClaim,
			action: "opened" | "restarted",
		) =>
			Effect.gen(function* () {
				const stored = claim.stored;
				const terminal = stored.terminal;
				const scope = yield* Scope.make();
				let transferred = false;

				return yield* Effect.gen(function* () {
					const opened = yield* driver
						.Open({
							args: terminal.args,
							cols: terminal.cols,
							cwd: terminal.working_directory,
							...(stored.env ? { env: stored.env } : {}),
							executable: terminal.executable,
							rows: terminal.rows,
						})
						.pipe(Scope.provide(scope), Effect.exit);

					if (Exit.isFailure(opened)) {
						const failure = failure_message(opened.cause);
						const commit = yield* repository.CommitCommand(
							command,
							claim.generation,
							"failed",
							{ _tag: "failed", failure },
						);

						return acceptance(commit, "accepted");
					}

					const handle = opened.value;

					return yield* Effect.gen(function* () {
						const commit = yield* repository.CommitCommand(
							command,
							claim.generation,
							action,
							{ _tag: "active", pid: handle.pid },
						);

						yield* RegisterLive(commit.stored, handle, scope);
						transferred = true;

						return acceptance(commit, "accepted");
					}).pipe(Effect.uninterruptible);
				}).pipe(
					Effect.ensuring(
						Effect.suspend(() =>
							transferred ? Effect.void : Scope.close(scope, Exit.void),
						),
					),
				);
			});

		const RecoverAmbiguous = (command: CommandEnvelope, claim: TerminalCommandClaim) =>
			Effect.gen(function* () {
				const terminal = claim.stored.terminal;
				const live = (yield* Ref.get(live_terminals)).get(terminal.terminal_id);

				if (live?.thread_id === command.thread_id && live.generation === claim.generation) {
					yield* live.handle.Close.pipe(Effect.ignore);
					yield* Deferred.await(live.done).pipe(Effect.ignore);
				}

				const commit = yield* repository.CommitAmbiguous(
					command,
					claim,
					"A previously claimed terminal command has an ambiguous dispatch result.",
				);

				return acceptance(commit, claim.status);
			});

		const Dispatch = (
			command: CommandEnvelope,
			payload: TerminalCommand,
			claim: TerminalCommandClaim,
		) => {
			if (payload.type === "terminal.open") {
				return Start(command, claim, "opened");
			}

			if (payload.type === "terminal.restart") {
				return Start(command, claim, "restarted");
			}

			if (payload.type === "terminal.pin") {
				return repository
					.CommitCommand(
						command,
						claim.generation,
						payload.pinned ? "pinned" : "unpinned",
						{
							_tag: "pin",
							pinned: payload.pinned,
						},
					)
					.pipe(Effect.map((commit) => acceptance(commit, "accepted")));
			}

			return Effect.gen(function* () {
				const live_map = yield* Ref.get(live_terminals);
				const live = live_map.get(payload.terminal_id);

				if (
					!live ||
					live.thread_id !== command.thread_id ||
					live.generation !== claim.generation
				) {
					return yield* RecoverAmbiguous(command, claim);
				}

				if (payload.type === "terminal.write") {
					yield* live.handle.Write(new TextEncoder().encode(payload.data));

					return acceptance(
						yield* repository.CommitCommand(command, claim.generation, "written", {
							_tag: "current",
						}),
						"accepted",
					);
				}

				if (payload.type === "terminal.resize") {
					yield* live.handle.Resize(payload.cols, payload.rows);

					return acceptance(
						yield* repository.CommitCommand(command, claim.generation, "resized", {
							_tag: "resize",
							cols: payload.cols,
							rows: payload.rows,
						}),
						"accepted",
					);
				}

				if (payload.type === "terminal.clear") {
					yield* live.handle.Clear;

					return acceptance(
						yield* repository.CommitCommand(command, claim.generation, "cleared", {
							_tag: "current",
						}),
						"accepted",
					);
				}

				if (payload.type === "terminal.kill") {
					yield* live.handle.Kill(payload.signal);
					yield* Deferred.await(live.done);

					return acceptance(
						yield* repository.CommitCommand(command, claim.generation, "killed", {
							_tag: "current",
						}),
						"accepted",
					);
				}

				yield* live.handle.Close;
				yield* Deferred.await(live.done);

				return acceptance(
					yield* repository.CommitCommand(command, claim.generation, "closed", {
						_tag: "current",
					}),
					"accepted",
				);
			}).pipe(
				Effect.catch((error) =>
					error instanceof TerminalDriverError
						? RecoverAmbiguous(command, claim)
						: Effect.fail(error),
				),
			);
		};

		const HandleUnlocked = (command: CommandEnvelope) =>
			Effect.gen(function* () {
				const payload = command.payload as TerminalCommand;

				if ((yield* Ref.get(quiesced_threads)).has(command.thread_id)) {
					return yield* new TerminalNotFound({ terminal_id: payload.terminal_id });
				}

				const claim = yield* repository.Claim(command, metadata.instance_id);

				if (claim.status === "duplicate") {
					const events = yield* journal.ReadCorrelatedEvents(command.message_id);

					if (claim.command_status === "dispatching") {
						return events.length > 0
							? yield* ReplayExisting(command, claim, events, true)
							: yield* RecoverAmbiguous(command, claim);
					}

					return yield* ReplayExisting(command, claim, events, false);
				}

				return yield* Dispatch(command, payload, claim);
			});

		const UserCommand = (command: CommandEnvelope): CommandEnvelope => {
			const { agent_id: _agent_id, run_id: _run_id, ...user_command } = command;

			return user_command;
		};
		const Handle = (command: CommandEnvelope) =>
			Semaphore.withPermit(command_lock)(HandleUnlocked(UserCommand(command)));
		const HandleAsAgent = (
			command: CommandEnvelope,
			authority: { readonly agent_id: string; readonly run_id: string },
		) =>
			Semaphore.withPermit(command_lock)(
				HandleUnlocked({ ...UserCommand(command), ...authority }),
			);

		const Recover = Effect.gen(function* () {
			const stale = yield* repository.ReadStale(metadata.instance_id);

			for (const stored of stale) {
				yield* repository.CommitRecovery(
					stored.terminal.terminal_id,
					stored.terminal.generation,
					"The backend stopped before this terminal lifecycle completed.",
				);
			}
		});
		const QuiesceThread = (thread_id: string) =>
			Semaphore.withPermit(command_lock)(
				Effect.gen(function* () {
					yield* Ref.update(quiesced_threads, (current) =>
						new Set(current).add(thread_id),
					);
					const terminals = [...(yield* Ref.get(live_terminals)).values()].filter(
						(live) => live.thread_id === thread_id,
					);

					yield* Effect.forEach(
						terminals,
						(live) => live.handle.Close.pipe(Effect.ignore),
						{ concurrency: "unbounded", discard: true },
					);
					yield* Effect.forEach(
						terminals,
						(live) => Deferred.await(live.done).pipe(Effect.ignore),
						{ concurrency: "unbounded", discard: true },
					);
				}),
			);

		const Shutdown = Effect.gen(function* () {
			const terminals = [...(yield* Ref.get(live_terminals)).values()];

			yield* Effect.forEach(terminals, (live) => live.handle.Close.pipe(Effect.ignore), {
				discard: true,
			});
			yield* Effect.forEach(
				terminals,
				(live) => Deferred.await(live.done).pipe(Effect.ignore),
				{ discard: true },
			);
			yield* Scope.close(service_scope, Exit.void);
		});

		yield* Effect.addFinalizer(() => Shutdown);
		yield* Recover;

		return {
			Handle,
			HandleAsAgent,
			List: repository.List,
			Output: (input) =>
				Ref.get(live_terminals).pipe(
					Effect.flatMap((terminals) => {
						const live = terminals.get(input.terminal_id);
						return live &&
							live.thread_id === input.thread_id &&
							live.workspace_id === input.workspace_id
							? Effect.succeed(live.handle.Output)
							: Effect.fail(
									new TerminalNotActive({ terminal_id: input.terminal_id }),
								);
					}),
				),
			ReadOutput: (terminal_id) =>
				Ref.get(live_terminals).pipe(
					Effect.flatMap((terminals) => {
						const live = terminals.get(terminal_id);
						return live
							? Effect.succeed(live.handle.Output)
							: Effect.fail(new TerminalNotActive({ terminal_id }));
					}),
				),
			QuiesceThread,
		};
	}),
);
