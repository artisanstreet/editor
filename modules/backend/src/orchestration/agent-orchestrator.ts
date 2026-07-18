import { Context, Deferred, Effect, Exit, Layer, Option, Ref, Scope, Stream } from "effect";

import { EngineRegistry, type EngineCommand, type EngineRun } from "@artisan/engines";
import type { CommandEnvelope } from "@artisan/protocol";

import {
	OrchestrationRepository,
	type AcceptedOrchestrationCommand,
	type OrchestrationError,
	type PendingWork,
} from "../persistence/orchestration-repository";
import { GlobalGuidanceService } from "../guidance/guidance-service";
import { MakeThreadDispatchFence } from "../threads/internal/thread-dispatch-fence";
import { IntakePolicy } from "./intake-policy";

interface LiveRun {
	readonly done: Deferred.Deferred<void>;
	readonly run: EngineRun;
	readonly scope: Scope.Scope;
	readonly thread_id: string;
}

type DispatchState = "idle" | "running" | "pending";

/** Coordinates durable thread work through registered provider-neutral engines. */
export class AgentOrchestrator extends Context.Service<
	AgentOrchestrator,
	{
		readonly Handle: (
			command: CommandEnvelope,
		) => Effect.Effect<AcceptedOrchestrationCommand, OrchestrationError>;
		readonly Recover: Effect.Effect<void, OrchestrationError>;
		readonly QuiesceThread: (thread_id: string) => Effect.Effect<void>;
	}
>()("Artisan/AgentOrchestrator") {}

export const AgentOrchestratorLive = Layer.effect(
	AgentOrchestrator,
	Effect.gen(function* () {
		const engines = yield* EngineRegistry;
		const guidance = yield* GlobalGuidanceService;
		const intake_policy = yield* IntakePolicy;
		const repository = yield* OrchestrationRepository;
		const service_scope = yield* Scope.make();
		const live_runs = yield* Ref.make(new Map<string, LiveRun>());
		const dispatch_state = yield* Ref.make<DispatchState>("idle");
		const dispatch_fence = yield* MakeThreadDispatchFence;

		const RemoveLiveRun = (run_id: string, run_scope: Scope.Scope) =>
			Effect.gen(function* () {
				const removed = yield* Ref.modify(live_runs, (runs) => {
					const live = runs.get(run_id);
					const next = new Map(runs);

					next.delete(run_id);

					return [live?.scope === run_scope, next] as const;
				});

				if (removed) {
					yield* Scope.close(run_scope, Exit.succeed(undefined));
				}
			});

		const CloseLiveRuns = Ref.modify(
			live_runs,
			(runs) => [[...runs.values()], new Map<string, LiveRun>()] as const,
		).pipe(
			Effect.flatMap((runs) =>
				Effect.forEach(runs, (live) => Scope.close(live.scope, Exit.succeed(undefined))),
			),
			Effect.asVoid,
		);

		yield* Effect.addFinalizer(() => CloseLiveRuns);
		yield* Effect.addFinalizer(() => Scope.close(service_scope, Exit.succeed(undefined)));

		const MarkStartFailure = (work: PendingWork) =>
			repository
				.RecordObservation({
					_tag: "run_terminal",
					artisan_run_id: work.run_id,
					observation_id: `open_failed:${work.command_id}`,
					raw: { engine_id: work.engine_id, frame: null, transport: "backend" },
					sequence: 0,
					state: "failed",
				})
				.pipe(
					Effect.catch(() => Effect.succeed([])),
					Effect.andThen(repository.MarkOutboxUndeliverable(work.command_id)),
				);

		const ObserveRun = (work: PendingWork, live: LiveRun) =>
			Stream.runForEach(live.run.Events, repository.RecordObservation).pipe(
				Effect.andThen(live.run.Closed),
				Effect.asVoid,
				Effect.catch(() => Effect.void),
				Effect.ensuring(
					Effect.gen(function* () {
						yield* Deferred.succeed(live.done, undefined);
						yield* RemoveLiveRun(work.run_id, live.scope);
					}),
				),
			);

		const StartRunUnfenced = (work: PendingWork) =>
			Effect.gen(function* () {
				if ((yield* Ref.get(live_runs)).has(work.run_id)) {
					return;
				}

				const claimed = yield* repository.ClaimOutbox(work.command_id);

				if (!claimed) {
					return;
				}

				const started = yield* repository.MarkRunStarted(work.run_id);

				if (started.length === 0 || work.payload.type !== "thread.send_message") {
					yield* repository.MarkOutboxUndeliverable(work.command_id);

					return;
				}

				const engine = yield* engines
					.Get(work.engine_id)
					.pipe(Effect.catch(() => Effect.succeed(undefined)));

				if (!engine) {
					yield* MarkStartFailure(work);

					return;
				}

				const resolved_guidance = yield* guidance
					.ResolveForEngine(work.engine_id)
					.pipe(Effect.exit);

				if (
					Exit.isFailure(resolved_guidance) ||
					(Option.isSome(resolved_guidance.value) &&
						engine.Descriptor.capabilities.global_guidance.state === "unsupported")
				) {
					yield* MarkStartFailure(work);

					return;
				}

				const run_scope = yield* Scope.make();
				const run = yield* engine
					.Open({
						_tag: "start",
						artisan_run_id: work.run_id,
						...(Option.isSome(resolved_guidance.value)
							? { global_guidance: resolved_guidance.value.value }
							: {}),
						initial_text: work.payload.text,
						working_directory: work.working_directory,
					})
					.pipe(
						Scope.provide(run_scope),
						Effect.catch(() => Effect.succeed(undefined)),
					);

				if (!run) {
					yield* Scope.close(run_scope, Exit.succeed(undefined));
					yield* MarkStartFailure(work);

					return;
				}

				const persisted = yield* repository
					.PersistNativeRun(work.run_id, run.native_thread_id, run.resume_token)
					.pipe(
						Effect.map(() => true),
						Effect.catch(() => Effect.succeed(false)),
					);

				if (!persisted) {
					yield* Scope.close(run_scope, Exit.succeed(undefined));
					yield* MarkStartFailure(work);

					return;
				}

				const done = yield* Deferred.make<void>();
				const live = {
					done,
					run,
					scope: run_scope,
					thread_id: work.thread_id,
				} satisfies LiveRun;
				yield* Ref.update(live_runs, (runs) => new Map(runs).set(work.run_id, live));
				const completed = yield* repository.CompleteOutbox(work.command_id).pipe(
					Effect.as(true),
					Effect.catch(() => Effect.succeed(false)),
				);

				if (!completed) {
					yield* RemoveLiveRun(work.run_id, run_scope);
					yield* MarkStartFailure(work);

					return;
				}

				yield* Effect.forkIn(ObserveRun(work, live), service_scope);
			});
		const StartRun = (work: PendingWork) =>
			dispatch_fence.Run(work.thread_id, StartRunUnfenced(work)).pipe(Effect.asVoid);

		const SendToLiveRunUnfenced = (work: PendingWork) =>
			Effect.gen(function* () {
				const claimed = yield* repository.ClaimOutbox(work.command_id);

				if (!claimed) {
					return;
				}

				const live = (yield* Ref.get(live_runs)).get(work.run_id);

				if (!live) {
					if (work.kind === "steer") {
						yield* repository.FallbackSteering(work.command_id);
					} else {
						yield* repository.MarkOutboxUndeliverable(work.command_id);
					}

					return;
				}

				const payload = work.payload;
				const command: EngineCommand | undefined =
					work.kind === "steer" && "text" in payload
						? { _tag: "steer", command_id: work.command_id, text: payload.text }
						: payload.type === "run.cancel"
							? { _tag: "cancel", command_id: work.command_id }
							: payload.type === "run.close"
								? { _tag: "close", command_id: work.command_id }
								: payload.type === "run.respond_approval"
									? {
											_tag: "respond_approval",
											approval_id: payload.approval_id,
											approved: payload.approved,
											command_id: work.command_id,
										}
									: payload.type === "run.respond_question"
										? {
												_tag: "respond_question",
												answers: payload.answers,
												command_id: work.command_id,
											}
										: undefined;

				if (!command) {
					yield* repository.MarkOutboxUndeliverable(work.command_id);

					return;
				}

				const delivered = yield* live.run.Send(command).pipe(
					Effect.as(true),
					Effect.catch(() => Effect.succeed(false)),
				);

				if (delivered) {
					yield* repository.CompleteOutbox(work.command_id);
				} else if (work.kind === "steer") {
					yield* repository.FallbackSteering(work.command_id);
				} else {
					yield* repository.MarkOutboxUndeliverable(work.command_id);
				}
			});
		const SendToLiveRun = (work: PendingWork) =>
			dispatch_fence.Run(work.thread_id, SendToLiveRunUnfenced(work)).pipe(Effect.asVoid);

		const DispatchPending = Effect.gen(function* () {
			const pending = yield* repository.GetPending();

			for (const work of pending) {
				if (work.kind === "start") {
					yield* StartRun(work);
				} else {
					yield* SendToLiveRun(work);
				}
			}
		});

		const DispatchLoop = Effect.gen(function* () {
			do {
				yield* DispatchPending;
			} while (
				yield* Ref.modify(
					dispatch_state,
					(state) =>
						[state === "pending", state === "pending" ? "running" : "idle"] as const,
				)
			);
		}).pipe(Effect.catch(() => Effect.asVoid(Ref.set(dispatch_state, "idle"))));

		const WakeDispatcher = Effect.gen(function* () {
			const start = yield* Ref.modify(dispatch_state, (state) =>
				state === "idle" ? ([true, "running"] as const) : ([false, "pending"] as const),
			);

			if (start) {
				yield* Effect.forkIn(DispatchLoop, service_scope);
			}
		});

		const CanSteer = (command: CommandEnvelope) =>
			command.payload.type !== "thread.send_message"
				? Effect.succeed(false)
				: repository.GetAutoSteer(command.thread_id).pipe(
						Effect.flatMap((enabled) =>
							enabled
								? repository.GetWork(command.thread_id).pipe(
										Effect.flatMap((work) => {
											if (
												!work ||
												(work.status !== "running" &&
													work.status !== "waiting")
											)
												return Effect.succeed(false);

											return engines.Get(work.engine_id).pipe(
												Effect.map(
													(engine) =>
														engine.Descriptor.capabilities.steer
															.state === "supported",
												),
												Effect.catch(() => Effect.succeed(false)),
											);
										}),
									)
								: Effect.succeed(false),
						),
					);

		const Handle = (command: CommandEnvelope) =>
			Effect.gen(function* () {
				const intake =
					command.payload.type === "thread.send_message"
						? yield* intake_policy.Assess(command.payload.text)
						: undefined;
				const accepted = yield* repository.Accept(
					command,
					yield* CanSteer(command),
					intake,
				);

				yield* WakeDispatcher;

				return accepted;
			});

		const Recover = Effect.gen(function* () {
			yield* repository.MarkInterrupted();
			yield* WakeDispatcher;
		});
		const QuiesceLiveRuns = (thread_id: string) =>
			Effect.gen(function* () {
				const runs = yield* Ref.modify(live_runs, (current) => {
					const matching = [...current.entries()].filter(
						([, live]) => live.thread_id === thread_id,
					);
					const next = new Map(current);

					for (const [run_id] of matching) {
						next.delete(run_id);
					}

					return [matching.map(([, live]) => live), next] as const;
				});

				yield* Effect.forEach(
					runs,
					(live) => Scope.close(live.scope, Exit.succeed(undefined)),
					{ concurrency: "unbounded", discard: true },
				);
				yield* Effect.forEach(runs, (live) => Deferred.await(live.done), {
					concurrency: "unbounded",
					discard: true,
				});
			});
		const QuiesceThread = (thread_id: string) =>
			dispatch_fence.Quiesce(thread_id, QuiesceLiveRuns(thread_id));

		yield* repository.MarkInterrupted();
		yield* WakeDispatcher;

		return { Handle, QuiesceThread, Recover };
	}),
);
