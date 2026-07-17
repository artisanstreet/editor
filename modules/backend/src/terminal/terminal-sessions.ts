import {
	Cause,
	Context,
	Deferred,
	Effect,
	Exit,
	Layer,
	Queue,
	Ref,
	Scope,
	Semaphore,
	Stream,
} from "effect";

import type { CommandEnvelope, EventEnvelope, TerminalSession } from "@artisan/protocol";

import { JournalStore, type JournalStoreError } from "../persistence/journal-store";
import { RuntimeMetadata } from "../runtime/runtime-metadata";
import {
	TerminalDriver,
	TerminalDriverError,
	type TerminalDriverExit,
	type TerminalDriverHandle,
} from "./terminal-driver";
import {
	TerminalInvariantError,
	TerminalNotActive,
	TerminalNotFound,
	TerminalRepository,
	type StoredTerminalSession,
	type TerminalCommand,
	type TerminalCommandClaim,
	type TerminalCommit,
	type TerminalLifecycleAction,
	type TerminalRepositoryError,
} from "./terminal-repository";

interface LiveTerminal {
	readonly done: Deferred.Deferred<StoredTerminalSession, TerminalSessionError>;
	readonly generation: number;
	readonly handle: TerminalDriverHandle;
	readonly output_state: Ref.Ref<LiveTerminalOutputState>;
	readonly output_pump_done: Deferred.Deferred<void>;
	readonly scope: Scope.Closeable;
	readonly thread_id: string;
}

interface LiveTerminalOutputState {
	readonly ended: boolean;
	readonly last_sequence: number;
	readonly viewers: ReadonlyMap<symbol, TerminalOutputViewer>;
}

interface TerminalOutputViewer {
	readonly queue: Queue.Queue<TerminalOutputChunk, Cause.Done<void>>;
}

interface OwnedTerminalOutputViewer extends TerminalOutputViewer {
	readonly thread_id: string;
}

type TerminalOutputDispatch =
	| { readonly _tag: "ended" }
	| {
			readonly _tag: "active";
			readonly sequence: number;
			readonly viewers: ReadonlyArray<TerminalOutputViewer>;
	  };

type TerminalOutputRegistration =
	| { readonly _tag: "ended" }
	| { readonly _tag: "active"; readonly expected_sequence: number };

interface RecentTerminalOutput {
	readonly bytes: Uint8Array;
	readonly generation: number;
	readonly thread_id: string;
	readonly was_truncated: boolean;
}

/** The largest terminal-output suffix retained for one live generation. */
export const TerminalRecentOutputMaxBytes = 65_536;

const terminal_output_viewer_capacity = 512;

/** One copied terminal output chunk in generation order. */
export interface TerminalOutputChunk {
	readonly _tag: "chunk";
	readonly data: Uint8Array;
	readonly sequence: number;
}

/** Reports the exact terminal output range evicted for one lagging viewer. */
export interface TerminalOutputGap {
	readonly _tag: "gap";
	readonly from_sequence: number;
	readonly reason: "viewer_overflow";
	readonly to_sequence: number;
}

/** Delivers terminal bytes with explicit per-viewer lag semantics. */
export type TerminalOutputEvent = TerminalOutputChunk | TerminalOutputGap;

/** Describes whether recent terminal output exists in this backend runtime. */
export interface TerminalRecentOutput {
	readonly output: Uint8Array;
	readonly state: "available" | "unavailable_after_restart";
	readonly terminal: TerminalSession;
	readonly truncated: boolean;
}

export type TerminalCommandEnvelope = CommandEnvelope & {
	readonly payload: TerminalCommand;
};

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
		readonly HandleCanonical: (
			command: TerminalCommandEnvelope,
			workspace_id: string,
		) => Effect.Effect<TerminalCommandAcceptance, TerminalSessionError>;
		readonly List: (
			thread_id: string,
			workspace_id: string,
		) => Effect.Effect<ReadonlyArray<TerminalSession>, TerminalSessionError>;
		readonly Output: (
			terminal_id: string,
			thread_id: string,
			workspace_id: string,
		) => Effect.Effect<Stream.Stream<TerminalOutputEvent>, TerminalSessionError>;
		readonly RecentOutput: (
			terminal_id: string,
			thread_id: string,
			workspace_id: string,
			max_bytes: number,
		) => Effect.Effect<TerminalRecentOutput, TerminalSessionError>;
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

function validate_recent_output_max_bytes(max_bytes: number) {
	return Number.isSafeInteger(max_bytes) &&
		max_bytes > 0 &&
		max_bytes <= TerminalRecentOutputMaxBytes
		? Effect.void
		: Effect.fail(
				new TerminalInvariantError({
					message: `Terminal recent-output max_bytes must be a positive safe integer no greater than ${TerminalRecentOutputMaxBytes}`,
				}),
			);
}

function append_recent_output(current: RecentTerminalOutput, chunk: Uint8Array) {
	const combined_length = current.bytes.length + chunk.length;
	const start = Math.max(0, combined_length - TerminalRecentOutputMaxBytes);
	const current_start = Math.min(current.bytes.length, start);
	const chunk_start = Math.max(0, start - current.bytes.length);
	const current_suffix = current.bytes.subarray(current_start);
	const chunk_suffix = chunk.subarray(chunk_start);
	const bytes = new Uint8Array(current_suffix.length + chunk_suffix.length);

	bytes.set(current_suffix);
	bytes.set(chunk_suffix, current_suffix.length);

	return { bytes, was_truncated: current.was_truncated || start > 0 };
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
		const output_viewers = yield* Ref.make(new Map<symbol, OwnedTerminalOutputViewer>());
		const recent_outputs = yield* Ref.make(new Map<string, RecentTerminalOutput>());
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
		const EndOutput = (output_state: Ref.Ref<LiveTerminalOutputState>) =>
			Effect.gen(function* () {
				const viewers = yield* Ref.modify(output_state, (current) => [
					[...current.viewers.values()],
					{ ...current, ended: true, viewers: new Map() },
				]);

				yield* Effect.forEach(viewers, ({ queue }) => Queue.end(queue), {
					concurrency: "unbounded",
					discard: true,
				});
			});
		const FenceOutput = (output_state: Ref.Ref<LiveTerminalOutputState>) =>
			Effect.gen(function* () {
				const viewers = yield* Ref.modify(output_state, (current) => [
					[...current.viewers.values()],
					{ ...current, ended: true, viewers: new Map() },
				]);

				yield* Effect.forEach(viewers, ({ queue }) => Queue.shutdown(queue), {
					concurrency: "unbounded",
					discard: true,
				});
			});
		const FenceThreadViewers = (thread_id: string) =>
			Effect.gen(function* () {
				const viewers = yield* Ref.modify(output_viewers, (current) => {
					const matching = [...current].filter(
						([, viewer]) => viewer.thread_id === thread_id,
					);
					const remaining = new Map(
						[...current].filter(([, viewer]) => viewer.thread_id !== thread_id),
					);

					return [matching.map(([, viewer]) => viewer), remaining] as const;
				});

				yield* Effect.forEach(viewers, ({ queue }) => Queue.shutdown(queue), {
					concurrency: "unbounded",
					discard: true,
				});
			});
		const FenceAllViewers = Effect.gen(function* () {
			const viewers = yield* Ref.modify(output_viewers, (current) => [
				[...current.values()],
				new Map(),
			]);

			yield* Effect.forEach(viewers, ({ queue }) => Queue.shutdown(queue), {
				concurrency: "unbounded",
				discard: true,
			});
		});

		const ObserveExit = (terminal_id: string, live: LiveTerminal) =>
			Effect.gen(function* () {
				const exit = yield* live.handle.Exit;

				yield* Effect.yieldNow;

				const output_drained = yield* Deferred.isDone(live.output_pump_done);
				const commit = output_drained
					? yield* repository.CommitExit(
							terminal_id,
							live.generation,
							exit,
							exit_action(exit.reason),
						)
					: yield* Effect.gen(function* () {
							yield* Scope.close(live.scope, Exit.void);

							return yield* repository.CommitRecovery(
								terminal_id,
								live.generation,
								"The terminal driver exited before its output stream completed.",
							);
						});

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
				const output_state = yield* Ref.make<LiveTerminalOutputState>({
					ended: false,
					last_sequence: 0,
					viewers: new Map(),
				});
				const output_pump_done = yield* Deferred.make<void>();
				const output_pump = handle.Output.pipe(
					Stream.runForEach((chunk) => {
						const copied_chunk = Uint8Array.from(chunk);

						return Effect.gen(function* () {
							const dispatch = yield* Ref.modify(
								output_state,
								(
									current,
								): readonly [TerminalOutputDispatch, LiveTerminalOutputState] => {
									if (current.ended) {
										return [{ _tag: "ended" }, current];
									}

									const sequence = current.last_sequence + 1;

									return [
										{
											_tag: "active" as const,
											sequence,
											viewers: [...current.viewers.values()],
										},
										{ ...current, last_sequence: sequence },
									];
								},
							);

							if (dispatch._tag === "ended") {
								return;
							}

							yield* Ref.update(recent_outputs, (outputs) => {
								const current = outputs.get(stored.terminal.terminal_id);

								if (
									!current ||
									current.generation !== stored.terminal.generation ||
									current.thread_id !== stored.terminal.thread_id
								) {
									return outputs;
								}

								return new Map(outputs).set(stored.terminal.terminal_id, {
									...append_recent_output(current, copied_chunk),
									generation: current.generation,
									thread_id: current.thread_id,
								});
							});
							yield* Effect.forEach(
								dispatch.viewers,
								({ queue }) =>
									Queue.offer(queue, {
										_tag: "chunk" as const,
										data: Uint8Array.from(copied_chunk),
										sequence: dispatch.sequence,
									}),
								{ concurrency: "unbounded", discard: true },
							);
						});
					}),
					Effect.ensuring(
						Effect.gen(function* () {
							yield* EndOutput(output_state);
							yield* Deferred.succeed(output_pump_done, undefined);
						}),
					),
				);
				const live = {
					done,
					generation: stored.terminal.generation,
					handle,
					output_state,
					output_pump_done,
					scope,
					thread_id: stored.terminal.thread_id,
				} satisfies LiveTerminal;

				yield* Ref.update(live_terminals, (terminals) =>
					new Map(terminals).set(stored.terminal.terminal_id, live),
				);
				yield* Ref.update(recent_outputs, (outputs) =>
					new Map(outputs).set(stored.terminal.terminal_id, {
						bytes: new Uint8Array(),
						generation: stored.terminal.generation,
						thread_id: stored.terminal.thread_id,
						was_truncated: false,
					}),
				);
				yield* Effect.forkIn(output_pump, scope);
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

		const Handle = (command: CommandEnvelope) =>
			Semaphore.withPermit(command_lock)(HandleUnlocked(command));
		const HandleCanonical = (command: TerminalCommandEnvelope, workspace_id: string) =>
			Semaphore.withPermit(command_lock)(
				Effect.gen(function* () {
					if (command.payload.workspace_id !== workspace_id) {
						return yield* new TerminalNotFound({
							terminal_id: command.payload.terminal_id,
						});
					}

					if (command.payload.type !== "terminal.open") {
						yield* repository.ReadOwned(
							command.payload.terminal_id,
							command.thread_id,
							command.payload.workspace_id,
						);
					}

					return yield* HandleUnlocked(command);
				}),
			);
		const Output = (terminal_id: string, thread_id: string, workspace_id: string) =>
			Semaphore.withPermit(command_lock)(
				Effect.gen(function* () {
					if ((yield* Ref.get(quiesced_threads)).has(thread_id)) {
						return yield* new TerminalNotFound({ terminal_id });
					}

					const stored = yield* repository.ReadOwned(
						terminal_id,
						thread_id,
						workspace_id,
					);
					const live = (yield* Ref.get(live_terminals)).get(terminal_id);

					if (
						!live ||
						live.thread_id !== stored.terminal.thread_id ||
						live.generation !== stored.terminal.generation
					) {
						return yield* new TerminalNotActive({ terminal_id });
					}

					const queue = yield* Queue.sliding<TerminalOutputChunk, Cause.Done<void>>(
						terminal_output_viewer_capacity,
					);
					const viewer_id = Symbol(terminal_id);
					const registration = yield* Ref.modify(
						live.output_state,
						(
							current,
						): readonly [TerminalOutputRegistration, LiveTerminalOutputState] => {
							if (current.ended) {
								return [{ _tag: "ended" }, current];
							}

							return [
								{
									_tag: "active" as const,
									expected_sequence: current.last_sequence + 1,
								},
								{
									...current,
									viewers: new Map(current.viewers).set(viewer_id, { queue }),
								},
							];
						},
					);

					if (registration._tag === "ended") {
						yield* Queue.shutdown(queue);

						return yield* new TerminalNotActive({ terminal_id });
					}

					yield* Ref.update(output_viewers, (current) =>
						new Map(current).set(viewer_id, { queue, thread_id }),
					);

					const RemoveViewer = Effect.gen(function* () {
						yield* Ref.update(live.output_state, (current) => {
							if (!current.viewers.has(viewer_id)) {
								return current;
							}

							const viewers = new Map(current.viewers);

							viewers.delete(viewer_id);

							return { ...current, viewers };
						});
						yield* Ref.update(output_viewers, (current) => {
							const viewers = new Map(current);

							viewers.delete(viewer_id);

							return viewers;
						});
						yield* Queue.shutdown(queue);
					});
					const stream = Stream.fromQueue(queue).pipe(
						Stream.mapAccum(
							() => registration.expected_sequence,
							(expected_sequence, chunk) => {
								const copied_chunk: TerminalOutputChunk = {
									...chunk,
									data: Uint8Array.from(chunk.data),
								};
								const values: ReadonlyArray<TerminalOutputEvent> =
									chunk.sequence === expected_sequence
										? [copied_chunk]
										: [
												{
													_tag: "gap",
													from_sequence: expected_sequence,
													reason: "viewer_overflow",
													to_sequence: chunk.sequence - 1,
												},
												copied_chunk,
											];

								return [chunk.sequence + 1, values] as const;
							},
						),
						Stream.ensuring(RemoveViewer),
					);

					return stream;
				}),
			);
		const RecentOutput = (
			terminal_id: string,
			thread_id: string,
			workspace_id: string,
			max_bytes: number,
		) =>
			Effect.gen(function* () {
				yield* validate_recent_output_max_bytes(max_bytes);

				const stored = yield* repository.ReadOwned(terminal_id, thread_id, workspace_id);
				const recent = (yield* Ref.get(recent_outputs)).get(terminal_id);

				if (
					!recent ||
					recent.generation !== stored.terminal.generation ||
					recent.thread_id !== stored.terminal.thread_id
				) {
					return {
						output: new Uint8Array(),
						state: "unavailable_after_restart" as const,
						terminal: stored.terminal,
						truncated: false,
					};
				}

				const start = Math.max(0, recent.bytes.length - max_bytes);

				return {
					output: Uint8Array.from(recent.bytes.subarray(start)),
					state: "available" as const,
					terminal: stored.terminal,
					truncated: recent.was_truncated || start > 0,
				};
			});

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
					yield* Effect.forEach(terminals, (live) => FenceOutput(live.output_state), {
						concurrency: "unbounded",
						discard: true,
					});
					yield* FenceThreadViewers(thread_id);
					yield* Ref.update(
						recent_outputs,
						(outputs) =>
							new Map(
								[...outputs].filter(([, recent]) => recent.thread_id !== thread_id),
							),
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
			yield* FenceAllViewers;
			yield* Ref.set(recent_outputs, new Map());
			yield* Scope.close(service_scope, Exit.void);
		});

		yield* Effect.addFinalizer(() => Shutdown);
		yield* Recover;

		return {
			Handle,
			HandleCanonical,
			List: repository.List,
			Output,
			RecentOutput,
			QuiesceThread,
		};
	}),
);
