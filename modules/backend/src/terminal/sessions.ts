import {
	Cause,
	Context,
	Deferred,
	Effect,
	Exit,
	Layer,
	Option,
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
	AppendScrollback,
	FinishScrollback,
	FollowScrollback,
	MakeTerminalScrollback,
	ReadScrollback,
	SnapshotScrollback,
	type TerminalScrollback,
} from "./scrollback";
import {
	AdoptObservedTerminalActivity,
	ObservedTerminalId,
	type ObservedTerminalActivity,
	type ObservedTerminalContext,
	type ObservedTerminalSettlement,
} from "./observed";
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
	readonly retired: Deferred.Deferred<void>;
	readonly scope: Scope.Closeable;
	readonly terminal_id: string;
	readonly thread_id: string;
	readonly workspace_id: string;
}

/** Unions terminal persistence and canonical journal failures. */
export type TerminalSessionError = JournalStoreError | TerminalRepositoryError;

/**
 * The recent-output window each terminal retains for replay. Sized for the
 * postmortem read — "what did the dev server log before it died" — rather than
 * a full transcript; older bytes are dropped silently.
 */
const SCROLLBACK_LIMIT_BYTES = 1_048_576;

/** How many exited terminals keep their scrollback readable before the oldest is dropped. */
const RETAINED_SCROLLBACK_TERMINALS = 16;

/**
 * A PTY close request is not proof that the host observed the process exit.
 * Keep command admission moving when a driver loses that notification, while
 * recording the lifecycle as ambiguous rather than inventing a clean close.
 */
const EXIT_SETTLEMENT_TIMEOUT = "1 second";

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
		readonly AdoptObserved: (
			activity: ObservedTerminalActivity,
			context: ObservedTerminalContext,
		) => Effect.Effect<void, TerminalSessionError>;
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
		readonly SettleObservedRun: (
			run_id: string,
			settlement: ObservedTerminalSettlement,
		) => Effect.Effect<void, TerminalSessionError>;
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
		const recovery = yield* Deferred.make<void, TerminalSessionError>();
		/**
		 * Scrollback outlives the live handle on purpose: the crash case — the
		 * moment the output matters most — is exactly when the handle is gone.
		 * Entries are replaced on restart and evicted oldest-finished-first.
		 */
		const scrollbacks = yield* Ref.make(new Map<string, TerminalScrollback>());
		const retired_scrollbacks = yield* Ref.make<
			ReadonlyArray<{
				readonly generation: number;
				readonly terminal_id: string;
				readonly thread_id: string;
			}>
		>([]);
		const output_encoder = new TextEncoder();

		const RetireScrollback = (terminal_id: string, scrollback: TerminalScrollback) =>
			Effect.gen(function* () {
				yield* FinishScrollback(scrollback);
				const evictions = yield* Ref.modify(retired_scrollbacks, (order) => {
					const appended = [
						...order.filter(
							(entry) =>
								entry.terminal_id !== terminal_id ||
								entry.generation !== scrollback.generation,
						),
						{
							generation: scrollback.generation,
							terminal_id,
							thread_id: scrollback.thread_id,
						},
					];
					const excess = appended.length - RETAINED_SCROLLBACK_TERMINALS;
					const evicted = excess > 0 ? appended.slice(0, excess) : [];
					const retained = excess > 0 ? appended.slice(excess) : appended;

					return [evicted, retained] as const;
				});

				if (evictions.length === 0) {
					return;
				}

				yield* Ref.update(scrollbacks, (current) => {
					const next = new Map(current);

					for (const eviction of evictions) {
						const retained = next.get(eviction.terminal_id);

						if (retained?.generation === eviction.generation) {
							next.delete(eviction.terminal_id);
						}
					}

					return next;
				});
			});

		const ObservedScrollback = (
			activity: ObservedTerminalActivity,
			context: ObservedTerminalContext,
		) =>
			Effect.gen(function* () {
				const terminal_id = ObservedTerminalId(activity.activity_id);
				const current = (yield* Ref.get(scrollbacks)).get(terminal_id);
				if (
					current?.thread_id === context.thread_id &&
					current.workspace_id === context.workspace_id
				) {
					return current;
				}

				const scrollback = yield* MakeTerminalScrollback({
					generation: 1,
					thread_id: context.thread_id,
					workspace_id: context.workspace_id,
				});
				yield* Ref.update(scrollbacks, (entries) =>
					new Map(entries).set(terminal_id, scrollback),
				);
				return scrollback;
			});

		const AppendObservedOutput = (
			scrollback: TerminalScrollback,
			activity: ObservedTerminalActivity,
		) =>
			Effect.gen(function* () {
				if (activity.output === undefined || activity.output.length === 0) return;
				const bytes = output_encoder.encode(activity.output);

				if (activity.state === "output") {
					yield* AppendScrollback(scrollback, bytes, SCROLLBACK_LIMIT_BYTES);
					return;
				}

				/**
				 * Codex repeats the full aggregate on completion after streaming deltas.
				 * Append only its unseen suffix; Claude reports output only on completion,
				 * so its empty scrollback receives the whole result.
				 */
				const retained = yield* ReadScrollback(scrollback);
				const extends_retained =
					bytes.length >= retained.length &&
					retained.every((byte, index) => bytes[index] === byte);
				if (!extends_retained) return;
				yield* AppendScrollback(
					scrollback,
					bytes.slice(retained.length),
					SCROLLBACK_LIMIT_BYTES,
				);
			});

		const AdoptObserved = (
			activity: ObservedTerminalActivity,
			context: ObservedTerminalContext,
		) =>
			Effect.gen(function* () {
				const terminal_id = ObservedTerminalId(activity.activity_id);
				const scrollback = yield* ObservedScrollback(activity, context);
				yield* AppendObservedOutput(scrollback, activity);

				const session = AdoptObservedTerminalActivity(activity, context);
				if (session !== undefined) {
					yield* repository.AdoptObserved(session, metadata.instance_id);
				}

				if (activity.state === "completed" || activity.state === "failed") {
					yield* RetireScrollback(terminal_id, scrollback);
				}
			});

		const SettleObservedRun = (run_id: string, settlement: ObservedTerminalSettlement) =>
			Effect.gen(function* () {
				const settled = yield* repository.SettleObservedRun(run_id, settlement);
				const retained = yield* Ref.get(scrollbacks);
				for (const terminal of settled) {
					const scrollback = retained.get(terminal.terminal_id);
					if (scrollback !== undefined) {
						yield* RetireScrollback(terminal.terminal_id, scrollback);
					}
				}
			});

		const PumpOutput = (
			terminal_id: string,
			scrollback: TerminalScrollback,
			handle: TerminalDriverHandle,
			retired: Deferred.Deferred<void>,
		) =>
			Effect.raceFirst(
				handle.Output.pipe(
					Stream.runForEach((chunk) =>
						AppendScrollback(scrollback, chunk, SCROLLBACK_LIMIT_BYTES),
					),
				),
				Deferred.await(retired),
			).pipe(Effect.ensuring(RetireScrollback(terminal_id, scrollback)));

		const ClaimLive = (terminal_id: string, generation: number) =>
			Ref.modify(live_terminals, (terminals) => {
				const current = terminals.get(terminal_id);

				if (!current || current.generation !== generation) {
					return [Option.none<LiveTerminal>(), terminals] as const;
				}

				const next = new Map(terminals);

				next.delete(terminal_id);

				return [Option.some(current), next] as const;
			});

		/** Releases an already claimed live generation after a lost exit notification. */
		const RetireClaimed = (live: LiveTerminal, exact_scrollback?: TerminalScrollback) =>
			Effect.gen(function* () {
				yield* Deferred.succeed(live.retired, undefined);
				yield* Deferred.interrupt(live.done);
				const scrollback =
					exact_scrollback ?? (yield* Ref.get(scrollbacks)).get(live.terminal_id);

				if (scrollback?.generation === live.generation) {
					yield* RetireScrollback(live.terminal_id, scrollback);
				}

				yield* Scope.close(live.scope, Exit.void).pipe(
					Effect.timeoutOption(EXIT_SETTLEMENT_TIMEOUT),
					Effect.ignore,
				);
			});

		const ObserveExit = (terminal_id: string, live: LiveTerminal) =>
			Effect.gen(function* () {
				const observed = yield* Effect.raceFirst(
					live.handle.Exit.pipe(Effect.map(Option.some)),
					Deferred.await(live.retired).pipe(Effect.as(Option.none())),
				);

				if (Option.isNone(observed)) {
					return;
				}
				const exit = observed.value;
				const claimed = yield* ClaimLive(terminal_id, live.generation);

				if (Option.isNone(claimed) || claimed.value !== live) {
					return;
				}
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
				const retired = yield* Deferred.make<void>();
				const live = {
					done,
					generation: stored.terminal.generation,
					handle,
					retired,
					scope,
					terminal_id: stored.terminal.terminal_id,
					thread_id: stored.terminal.thread_id,
					workspace_id: stored.terminal.workspace_id,
				} satisfies LiveTerminal;
				const scrollback = yield* MakeTerminalScrollback({
					generation: stored.terminal.generation,
					thread_id: stored.terminal.thread_id,
					workspace_id: stored.terminal.workspace_id,
				});

				const displaced_live = yield* Ref.modify(live_terminals, (terminals) => {
					const next = new Map(terminals);
					const displaced = next.get(stored.terminal.terminal_id);

					next.set(stored.terminal.terminal_id, live);

					return [displaced, next] as const;
				});
				const displaced_scrollback = yield* Ref.modify(scrollbacks, (current) => {
					const next = new Map(current);
					const displaced = next.get(stored.terminal.terminal_id);

					next.set(stored.terminal.terminal_id, scrollback);

					return [displaced, next] as const;
				});

				if (displaced_live && displaced_live.generation !== live.generation) {
					yield* Effect.forkIn(
						RetireClaimed(
							displaced_live,
							displaced_scrollback?.generation === displaced_live.generation
								? displaced_scrollback
								: undefined,
						),
						service_scope,
						{ startImmediately: true },
					);
				}
				yield* Effect.forkIn(
					PumpOutput(stored.terminal.terminal_id, scrollback, handle, retired),
					service_scope,
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
							{
								_tag: "failed",
								failure,
							},
						);

						return acceptance(commit, "accepted");
					}

					const handle = opened.value;

					return yield* Effect.gen(function* () {
						const commit = yield* repository.CommitCommand(
							command,
							claim.generation,
							action,
							{
								_tag: "active",
								pid: handle.pid,
							},
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

		const RecoverAmbiguous = (
			command: CommandEnvelope,
			claim: TerminalCommandClaim,
			stop_already_requested = false,
			retire_unsettled = false,
		) =>
			Effect.gen(function* () {
				const terminal = claim.stored.terminal;
				const live = (yield* Ref.get(live_terminals)).get(terminal.terminal_id);

				if (live?.thread_id === command.thread_id && live.generation === claim.generation) {
					if (!stop_already_requested) {
						yield* live.handle.Close.pipe(Effect.ignore);
						const settled = yield* AwaitExit(live).pipe(Effect.option);

						if (Option.isNone(settled) || Option.isNone(settled.value)) {
							yield* RetireIfCurrent(live);
						}
					} else if (retire_unsettled) {
						yield* RetireIfCurrent(live);
					}
				}

				const commit = yield* repository.CommitAmbiguous(
					command,
					claim,
					"A previously claimed terminal command has an ambiguous dispatch result.",
				);

				return acceptance(commit, claim.status);
			});

		const AwaitExit = (live: LiveTerminal) =>
			Deferred.await(live.done).pipe(Effect.timeoutOption(EXIT_SETTLEMENT_TIMEOUT));

		const RetireIfCurrent = (live: LiveTerminal) =>
			ClaimLive(live.terminal_id, live.generation).pipe(
				Effect.flatMap((claimed) =>
					Option.isSome(claimed) && claimed.value === live
						? RetireClaimed(live)
						: Effect.void,
				),
			);

		const WatchExit = (live: LiveTerminal) =>
			Effect.gen(function* () {
				const settled = yield* AwaitExit(live).pipe(Effect.exit);

				if (Exit.isFailure(settled) || Option.isSome(settled.value)) {
					return;
				}
				const claimed = yield* ClaimLive(live.terminal_id, live.generation);

				if (Option.isNone(claimed) || claimed.value !== live) {
					return;
				}
				const recovery = yield* repository
					.CommitRecovery(
						live.terminal_id,
						live.generation,
						"The terminal driver did not publish an exit after a stop request.",
					)
					.pipe(Effect.exit);

				if (Exit.isFailure(recovery)) {
					yield* Deferred.failCause(live.done, recovery.cause);
				} else {
					yield* Deferred.succeed(live.done, recovery.value.stored);
				}
				yield* RetireClaimed(live);
			});

		const CloseLive = (live: LiveTerminal) =>
			live.handle.Close.pipe(
				Effect.ignore,
				Effect.andThen(AwaitExit(live)),
				Effect.flatMap((settled) =>
					Option.isNone(settled) ? RetireIfCurrent(live) : Effect.void,
				),
				Effect.catch(() => RetireIfCurrent(live)),
			);

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
					const commit = yield* repository.CommitCommand(
						command,
						claim.generation,
						"killed",
						{ _tag: "current" },
					);

					yield* Effect.forkIn(WatchExit(live), service_scope, {
						startImmediately: true,
					});

					return acceptance(commit, "accepted");
				}

				yield* live.handle.Close;
				const commit = yield* repository.CommitCommand(
					command,
					claim.generation,
					"closed",
					{
						_tag: "current",
					},
				);

				yield* Effect.forkIn(WatchExit(live), service_scope, { startImmediately: true });

				return acceptance(commit, "accepted");
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
			Deferred.await(recovery).pipe(
				Effect.andThen(
					Semaphore.withPermit(command_lock)(HandleUnlocked(UserCommand(command))),
				),
			);
		const HandleAsAgent = (
			command: CommandEnvelope,
			authority: { readonly agent_id: string; readonly run_id: string },
		) =>
			Deferred.await(recovery).pipe(
				Effect.andThen(
					Semaphore.withPermit(command_lock)(
						HandleUnlocked({ ...UserCommand(command), ...authority }),
					),
				),
			);

		const Recover = repository.RecoverStale(
			metadata.instance_id,
			"The backend stopped before this terminal lifecycle completed.",
		);
		const QuiesceThread = (thread_id: string) =>
			Deferred.await(recovery).pipe(
				Effect.andThen(
					Semaphore.withPermit(command_lock)(
						Effect.gen(function* () {
							yield* Ref.update(quiesced_threads, (current) =>
								new Set(current).add(thread_id),
							);
							const terminals = [...(yield* Ref.get(live_terminals)).values()].filter(
								(live) => live.thread_id === thread_id,
							);

							yield* Effect.forEach(terminals, CloseLive, {
								concurrency: "unbounded",
								discard: true,
							});
							yield* Ref.update(
								scrollbacks,
								(current) =>
									new Map(
										[...current].filter(
											([, scrollback]) => scrollback.thread_id !== thread_id,
										),
									),
							);
							yield* Ref.update(retired_scrollbacks, (order) =>
								order.filter((entry) => entry.thread_id !== thread_id),
							);
						}),
					),
				),
			);

		const Shutdown = Effect.gen(function* () {
			const terminals = [...(yield* Ref.get(live_terminals)).values()];

			yield* Effect.forEach(terminals, CloseLive, {
				concurrency: "unbounded",
				discard: true,
			});
			yield* Scope.close(service_scope, Exit.void);
		});

		yield* Effect.addFinalizer(() =>
			Deferred.interrupt(recovery).pipe(Effect.andThen(Shutdown)),
		);
		yield* Effect.forkIn(Deferred.complete(recovery, Recover), service_scope, {
			startImmediately: true,
		});

		return {
			AdoptObserved,
			Handle,
			HandleAsAgent,
			List: repository.List,
			Output: (input) =>
				Ref.get(scrollbacks).pipe(
					Effect.flatMap((entries) => {
						const scrollback = entries.get(input.terminal_id);
						return scrollback &&
							scrollback.thread_id === input.thread_id &&
							scrollback.workspace_id === input.workspace_id
							? Effect.succeed(FollowScrollback(scrollback))
							: Effect.fail(
									new TerminalNotActive({ terminal_id: input.terminal_id }),
								);
					}),
				),
			ReadOutput: (terminal_id) =>
				Ref.get(scrollbacks).pipe(
					Effect.flatMap((entries) => {
						const scrollback = entries.get(terminal_id);
						return scrollback
							? SnapshotScrollback(scrollback)
							: Effect.fail(new TerminalNotActive({ terminal_id }));
					}),
				),
			SettleObservedRun,
			QuiesceThread,
		};
	}),
);
