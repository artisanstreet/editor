import { Cause, Context, Deferred, Effect, Exit, Layer, Option, Ref, Scope, Stream } from "effect";

import {
	EngineRegistry,
	type EngineCommand,
	type EngineRun,
	type EngineUserInputPart,
} from "@artisan/engines";
import type { CommandEnvelope } from "@artisan/protocol";

import {
	OrchestrationRepository,
	type AcceptedOrchestrationCommand,
	type OrchestrationError,
	type PendingWork,
	type RecoverableNativeRun,
} from "../persistence/orchestration-repository";
import type {
	AuthoritativeCommandEnvelope,
	AuthoritativeThreadSendMessageCommand,
	InboundOrAuthoritativeCommandEnvelope,
} from "../persistence/orchestration/message-command";
import { GlobalGuidanceService } from "../guidance/guidance-service";
import { MakeThreadDispatchFence } from "../threads/internal/thread-dispatch-fence";
import { IntakePolicy } from "./intake-policy";
import { IsSessionPolicyEngine, MakeSessionPolicyRunMetadata } from "./session-policy";

interface LiveRun {
	readonly done: Deferred.Deferred<void>;
	readonly run: EngineRun;
	readonly scope: Scope.Scope;
	readonly thread_id: string;
}

type DispatchState = "idle" | "running" | "pending";

/** Bounds the complete multi-request native startup while exceeding Codex's nested request budgets. */
const engine_open_timeout_ms = 60_000;

type StartFailureKind = "configuration" | "engine_error" | "interrupted" | "timeout";

const InitialContent = (payload: AuthoritativeThreadSendMessageCommand) => {
	if (payload.content === undefined) return undefined;
	const attachments = new Map(
		(payload.attachments ?? []).map((attachment) => [attachment.id, attachment]),
	);
	const content: Array<EngineUserInputPart> = [];
	for (const part of payload.content) {
		if (part.type === "text") {
			content.push(part);
			continue;
		}
		const attachment = attachments.get(part.attachment_id);
		if (attachment?.bytes === undefined) continue;
		content.push({
			bytes: attachment.bytes,
			id: attachment.id,
			media_type: attachment.media_type,
			name: attachment.name,
			type: "image",
		});
	}
	return content;
};

interface StartFailure {
	readonly diagnostic?: string;
	readonly kind: StartFailureKind;
	readonly message: string;
}

function tagged_failure_name(cause: Cause.Cause<unknown>) {
	const failure = Cause.findErrorOption(cause);

	if (Option.isNone(failure)) {
		return undefined;
	}

	const value = failure.value;
	if (
		value === null ||
		typeof value !== "object" ||
		!("_tag" in value) ||
		typeof value._tag !== "string"
	) {
		return undefined;
	}

	return /^[A-Za-z][A-Za-z0-9_.-]{0,63}$/.test(value._tag) ? value._tag : undefined;
}

function start_failure_from_cause(cause: Cause.Cause<unknown>): StartFailure {
	if (Cause.hasInterruptsOnly(cause)) {
		return {
			kind: "interrupted",
			message: "Engine startup was interrupted before the native session became ready.",
		};
	}

	const failure_name = tagged_failure_name(cause);

	return {
		diagnostic: Cause.pretty(cause),
		kind: "engine_error",
		message:
			failure_name === undefined
				? "Engine startup failed before the native session became ready."
				: `Engine startup failed before the native session became ready (${failure_name}).`,
	};
}

/** Coordinates durable thread work through registered provider-neutral engines. */
export class AgentOrchestrator extends Context.Service<
	AgentOrchestrator,
	{
		readonly Handle: (
			command: AuthoritativeCommandEnvelope,
		) => Effect.Effect<AcceptedOrchestrationCommand, OrchestrationError>;
		readonly HandleInbound: (
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

		const MarkStartFailure = (work: PendingWork, failure: StartFailure) =>
			Effect.gen(function* () {
				yield* repository
					.RecordObservation({
						_tag: "process_diagnostic",
						artisan_run_id: work.run_id,
						level: "error",
						message: failure.message,
						observation_id: `open_diagnostic:${work.command_id}`,
						raw: {
							engine_id: work.engine_id,
							frame: failure.kind,
							transport: "backend",
						},
						sequence: 0,
					})
					.pipe(Effect.catchCause(() => Effect.succeed([])));
				yield* repository
					.RecordObservation({
						_tag: "run_terminal",
						artisan_run_id: work.run_id,
						observation_id: `open_failed:${work.command_id}`,
						raw: {
							engine_id: work.engine_id,
							frame: failure.kind,
							transport: "backend",
						},
						sequence: 1,
						state: "failed",
					})
					.pipe(Effect.catchCause(() => Effect.succeed([])));
				yield* repository
					.MarkOutboxUndeliverable(work.command_id)
					.pipe(Effect.catchCause(() => Effect.void));

				yield* Effect.sync(() => {
					console.error("Artisan engine startup failed", {
						command_id: work.command_id,
						...(failure.diagnostic === undefined
							? {}
							: { diagnostic: failure.diagnostic }),
						engine_id: work.engine_id,
						failure_kind: failure.kind,
						run_id: work.run_id,
					});
				});
			});

		const ObserveRun = (work: Pick<PendingWork, "run_id">, live: LiveRun) =>
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

		const StartClaimedRun = (work: PendingWork) =>
			Effect.gen(function* () {
				const started = yield* repository.MarkRunStarted(work.run_id);

				if (started.length === 0 || work.payload.type !== "thread.send_message") {
					yield* MarkStartFailure(work, {
						kind: "configuration",
						message:
							"Engine startup was rejected because the queued command was invalid.",
					});

					return;
				}

				const engine = yield* engines
					.Get(work.engine_id)
					.pipe(Effect.catch(() => Effect.succeed(undefined)));

				if (!engine) {
					yield* MarkStartFailure(work, {
						kind: "configuration",
						message:
							"Engine startup failed because the selected engine is unavailable.",
					});

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
					yield* MarkStartFailure(work, {
						kind: "configuration",
						message:
							"Engine startup failed because its global guidance is unavailable.",
					});

					return;
				}

				const run_scope = yield* Scope.make();
				const policy = yield* repository.GetSessionPolicy(work.thread_id);
				if (
					!IsSessionPolicyEngine(policy, work.engine_id) &&
					engine.Descriptor.transport !== "test"
				) {
					yield* Scope.close(run_scope, Exit.succeed(undefined));
					yield* MarkStartFailure(work, {
						kind: "configuration",
						message:
							"Engine startup failed because the thread policy targets another engine.",
					});

					return;
				}
				const initial_content = InitialContent(work.payload);
				const open_exit = yield* engine
					.Open({
						_tag: "start",
						artisan_run_id: work.run_id,
						...(Option.isSome(resolved_guidance.value)
							? { global_guidance: resolved_guidance.value.value }
							: {}),
						initial_text: work.payload.text,
						...(initial_content === undefined ? {} : { initial_content }),
						...MakeSessionPolicyRunMetadata(policy),
						working_directory: work.working_directory,
					})
					.pipe(
						Scope.provide(run_scope),
						Effect.timeoutOption(engine_open_timeout_ms),
						Effect.exit,
					);

				if (Exit.isFailure(open_exit)) {
					yield* Scope.close(run_scope, Exit.succeed(undefined));
					yield* MarkStartFailure(work, start_failure_from_cause(open_exit.cause));

					return;
				}
				const run = open_exit.value;

				if (Option.isNone(run)) {
					yield* Scope.close(run_scope, Exit.succeed(undefined));
					yield* MarkStartFailure(work, {
						kind: "timeout",
						message: `Engine startup timed out after ${engine_open_timeout_ms / 1_000} seconds.`,
					});

					return;
				}
				const opened_run = run.value;

				const persisted = yield* repository
					.PersistNativeRun(
						work.run_id,
						opened_run.native_thread_id,
						opened_run.resume_token,
					)
					.pipe(
						Effect.map(() => true),
						Effect.catch(() => Effect.succeed(false)),
					);

				if (!persisted) {
					yield* Scope.close(run_scope, Exit.succeed(undefined));
					yield* MarkStartFailure(work, {
						kind: "configuration",
						message: "Engine startup failed while saving the native session identity.",
					});

					return;
				}

				const done = yield* Deferred.make<void>();
				const live = {
					done,
					run: opened_run,
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
					yield* MarkStartFailure(work, {
						kind: "configuration",
						message: "Engine startup failed while completing its durable dispatch.",
					});

					return;
				}

				yield* Effect.forkIn(ObserveRun(work, live), service_scope);
			});
		const StartRunUnfenced = (work: PendingWork) =>
			Effect.gen(function* () {
				if ((yield* Ref.get(live_runs)).has(work.run_id)) {
					return;
				}

				const claimed = yield* repository.ClaimOutbox(work.command_id);

				if (!claimed) {
					return;
				}

				yield* StartClaimedRun(work).pipe(
					Effect.catchCause((cause) =>
						MarkStartFailure(work, start_failure_from_cause(cause)),
					),
				);
			});
		const StartRun = (work: PendingWork) =>
			dispatch_fence.Run(work.thread_id, StartRunUnfenced(work)).pipe(Effect.asVoid);

		const ResumeRunUnfenced = (work: RecoverableNativeRun) =>
			Effect.gen(function* () {
				if ((yield* Ref.get(live_runs)).has(work.run_id)) {
					return;
				}

				const engine = yield* engines
					.Get(work.engine_id)
					.pipe(Effect.catch(() => Effect.succeed(undefined)));

				if (!engine || engine.Descriptor.capabilities.resume.state === "unsupported") {
					return;
				}

				const policy = yield* repository.GetSessionPolicy(work.thread_id);
				if (
					!IsSessionPolicyEngine(policy, work.engine_id) &&
					engine.Descriptor.transport !== "test"
				) {
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
					return;
				}

				const run_scope = yield* Scope.make();
				const run = yield* engine
					.Open({
						_tag: "resume",
						artisan_run_id: work.run_id,
						...(Option.isSome(resolved_guidance.value)
							? { global_guidance: resolved_guidance.value.value }
							: {}),
						...MakeSessionPolicyRunMetadata(policy),
						resume_token: work.resume_token,
						working_directory: work.working_directory,
					})
					.pipe(
						Scope.provide(run_scope),
						Effect.catch(() => Effect.succeed(undefined)),
					);

				if (!run) {
					yield* Scope.close(run_scope, Exit.succeed(undefined));

					return;
				}

				const persisted = yield* repository
					.PersistNativeRun(work.run_id, run.native_thread_id, run.resume_token)
					.pipe(
						Effect.as(true),
						Effect.catch(() => Effect.succeed(false)),
					);
				const resumed = persisted
					? yield* repository
							.MarkRunResumed(work.run_id)
							.pipe(Effect.catch(() => Effect.succeed(false)))
					: false;

				if (!resumed) {
					yield* Scope.close(run_scope, Exit.succeed(undefined));

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
				yield* Effect.forkIn(ObserveRun(work, live), service_scope);
			});
		const ResumeRun = (work: RecoverableNativeRun) =>
			dispatch_fence.Run(work.thread_id, ResumeRunUnfenced(work)).pipe(Effect.asVoid);

		const SendToLiveRunUnfenced = (work: PendingWork) =>
			Effect.gen(function* () {
				const claimed = yield* repository.ClaimOutbox(work.command_id);

				if (!claimed) {
					return;
				}

				const live = (yield* Ref.get(live_runs)).get(work.run_id);

				if (!live) {
					if (work.kind === "steer") {
						yield* repository.FallbackSteering(work.command_id, "rejected");
					} else {
						yield* repository.MarkOutboxUndeliverable(work.command_id);
					}

					return;
				}

				const payload = work.payload;
				const command: EngineCommand | undefined =
					work.kind === "steer" && "text" in payload
						? {
								_tag: "steer",
								command_id: work.command_id,
								content:
									payload.type === "thread.send_message"
										? InitialContent(payload)
										: undefined,
								text: payload.text,
							}
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
					yield* repository.FallbackSteering(work.command_id, "delivery_failed");
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

		const DispatchLoop: Effect.Effect<void> = Effect.suspend(() =>
			Effect.gen(function* () {
				do {
					yield* DispatchPending;
				} while (
					yield* Ref.modify(
						dispatch_state,
						(state) =>
							[
								state === "pending",
								state === "pending" ? "running" : "idle",
							] as const,
					)
				);
			}).pipe(
				Effect.catchCause((cause) =>
					Effect.sync(() => {
						console.error("Artisan dispatcher recovered from an unexpected cause", {
							failure_kind: Cause.hasInterruptsOnly(cause) ? "interrupted" : "defect",
						});
					}),
				),
				Effect.ensuring(
					Effect.gen(function* () {
						const should_restart = yield* Ref.modify(dispatch_state, (state) =>
							state === "pending"
								? ([true, "running"] as const)
								: ([false, "idle"] as const),
						);

						if (should_restart) {
							yield* Effect.forkIn(DispatchLoop, service_scope);
						}
					}),
				),
			),
		);

		const WakeDispatcher = Effect.gen(function* () {
			const start = yield* Ref.modify(dispatch_state, (state) =>
				state === "idle" ? ([true, "running"] as const) : ([false, "pending"] as const),
			);

			if (start) {
				yield* Effect.forkIn(DispatchLoop, service_scope);
			}
		});

		type RoutingDecision =
			| { readonly can_steer: true }
			| {
					readonly can_steer: false;
					readonly reason:
						| "no_active_run"
						| "disabled"
						| "unsupported"
						| "ambiguous_target";
			  };
		const CanSteer = (
			command: InboundOrAuthoritativeCommandEnvelope,
		): Effect.Effect<RoutingDecision, OrchestrationError> =>
			Effect.gen(function* () {
				if (command.payload.type !== "thread.send_message") {
					return { can_steer: false, reason: "no_active_run" } as const;
				}
				const requested_engine_id = command.payload.engine_id;
				if (!(yield* repository.GetAutoSteer(command.thread_id))) {
					return { can_steer: false, reason: "disabled" } as const;
				}
				const work = yield* repository.GetWork(command.thread_id);
				if (!work || (work.status !== "running" && work.status !== "waiting")) {
					return { can_steer: false, reason: "no_active_run" } as const;
				}
				if (work.engine_id !== requested_engine_id) {
					return { can_steer: false, reason: "ambiguous_target" } as const;
				}
				const supported = yield* engines.Get(work.engine_id).pipe(
					Effect.map(
						(engine) => engine.Descriptor.capabilities.steer.state === "supported",
					),
					Effect.catch(() => Effect.succeed(false)),
				);
				return supported
					? ({ can_steer: true } as const)
					: ({ can_steer: false, reason: "unsupported" } as const);
			});

		const HandleCommand = (command: InboundOrAuthoritativeCommandEnvelope, inbound: boolean) =>
			Effect.gen(function* () {
				const initial_intake =
					command.payload.type === "thread.send_message"
						? yield* intake_policy.Assess(command.payload.text)
						: undefined;
				const policy =
					command.payload.type === "thread.send_message"
						? yield* repository.GetSessionPolicy(command.thread_id)
						: undefined;
				const intake =
					initial_intake &&
					policy?.strict_clarification &&
					initial_intake.assumptions.length > 0
						? {
								...initial_intake,
								resolution: "question" as const,
								question:
									"Please confirm the intended scope before Artisan proceeds.",
								assumptions: [],
							}
						: initial_intake;
				const routing = yield* CanSteer(command);
				const accepted = yield* inbound
					? repository.AcceptInbound(
							command as CommandEnvelope,
							routing.can_steer,
							intake,
							"reason" in routing ? routing.reason : undefined,
						)
					: repository.Accept(
							command as AuthoritativeCommandEnvelope,
							routing.can_steer,
							intake,
							"reason" in routing ? routing.reason : undefined,
						);

				yield* WakeDispatcher;

				return accepted;
			});
		const Handle = (command: AuthoritativeCommandEnvelope) => HandleCommand(command, false);
		const HandleInbound = (command: CommandEnvelope) => HandleCommand(command, true);

		const Recover = Effect.gen(function* () {
			/** Recovery is a cold-start operation. Never interrupt an in-memory run merely because a caller re-reads recovery state. */
			if ((yield* Ref.get(live_runs)).size > 0) {
				yield* WakeDispatcher;

				return;
			}

			const recoverable = yield* repository.ClaimNativeRecoveries();
			yield* Effect.forEach(recoverable, ResumeRun, {
				concurrency: "unbounded",
				discard: true,
			});
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

		yield* Recover;

		return { Handle, HandleInbound, QuiesceThread, Recover };
	}),
);
