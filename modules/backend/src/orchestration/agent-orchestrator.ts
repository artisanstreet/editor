import {
	Cause,
	Context,
	Deferred,
	Effect,
	Exit,
	Layer,
	Option,
	PubSub,
	Ref,
	Scope,
	Stream,
} from "effect";

import { artisan_error_codes } from "@artisan/catalog";
import {
	EngineRegistry,
	type EngineCommand,
	type EngineGlobalGuidance,
	type EngineObservation,
	type EngineOpenInput,
	type EngineRun,
	type EngineUserInputPart,
} from "@artisan/engines";
import type { CommandEnvelope } from "@artisan/protocol";

import {
	OrchestrationRepository,
	OrchestrationFailure,
	type AcceptedOrchestrationCommand,
	type OrchestrationError,
	type PendingWork,
	type RecoverableNativeRun,
} from "../persistence/orchestration/repository";
import { ThreadContinuationRepository } from "../persistence/thread-continuation/repository";
import type {
	AuthoritativeCommandEnvelope,
	AuthoritativeThreadSendMessageCommand,
	InboundOrAuthoritativeCommandEnvelope,
} from "../persistence/orchestration/message-command";
import { GlobalGuidanceService } from "../guidance/service";
import { HostSuspendMonitor } from "../host/suspend-monitor";
import { MakeThreadDispatchFence } from "../threads/internal/thread-dispatch-fence";
import { AgentGraphRepository } from "./agent-graph-repository";
import {
	start_failure_from_cause,
	type StartFailure,
} from "./internal/agent-orchestrator-start-failure";
import { IntakePolicy } from "./intake-policy";
import { MakeObservationPersistence } from "./observation-persistence";
import { ProductInstructions } from "./product-instructions";
import {
	render_portable_checkpoint_context,
	render_portable_checkpoint_prompt,
} from "./thread-continuation-model";
import {
	ThreadContinuationService,
	type PreparedThreadContinuation,
} from "./thread-continuation-service";
import {
	IsSessionPolicyEngine,
	MakeSessionPolicyRunMetadata,
	SessionPolicyResolvedModel,
} from "./session-policy";

interface LiveRun {
	readonly done: Deferred.Deferred<void>;
	readonly run: EngineRun;
	readonly scope: Scope.Scope;
	readonly thread_id: string;
}

type DispatchState = "idle" | "running" | "pending";

/** Bounds the complete multi-request native startup while exceeding Codex's nested request budgets. */
const engine_open_timeout_ms = 60_000;

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

const PublicObservation = (observation: EngineObservation): EngineObservation => {
	if (observation._tag !== "compaction" || observation.summary === undefined) return observation;
	const { summary: _private_summary, ...public_observation } = observation;
	return public_observation;
};

const MakeEngineOpenInput = (
	work: PendingWork & { readonly payload: AuthoritativeThreadSendMessageCommand },
	prepared: PreparedThreadContinuation,
	run_metadata: ReturnType<typeof MakeSessionPolicyRunMetadata>,
	global_guidance: EngineGlobalGuidance | undefined,
): EngineOpenInput => {
	const content = InitialContent(work.payload);
	const common = {
		artisan_run_id: work.run_id,
		...(global_guidance === undefined ? {} : { global_guidance }),
		...run_metadata,
		working_directory: work.working_directory,
	};

	if (prepared._tag === "native") {
		return {
			...common,
			_tag: "resume" as const,
			...(content === undefined ? {} : { next_content: content }),
			next_text: work.payload.text,
			resume_token: prepared.resume_token,
		};
	}

	if (prepared._tag === "portable") {
		return content === undefined
			? {
					...common,
					_tag: "start" as const,
					initial_text: render_portable_checkpoint_prompt({
						checkpoint: prepared.checkpoint,
						current_request: work.payload.text,
					}),
				}
			: {
					...common,
					_tag: "start" as const,
					initial_content: [
						{
							text: render_portable_checkpoint_context(prepared.checkpoint),
							type: "text" as const,
						},
						...content,
					],
					// Adapters use ordered content when it is present; this required field is not duplicated.
					initial_text: work.payload.text,
				};
	}

	return {
		...common,
		_tag: "start" as const,
		...(content === undefined ? {} : { initial_content: content }),
		initial_text: work.payload.text,
	};
};

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
		const product_instructions = yield* ProductInstructions;
		const intake_policy = yield* IntakePolicy;
		const repository = yield* OrchestrationRepository;
		const graph_repository = yield* AgentGraphRepository;
		const continuation = yield* ThreadContinuationService;
		const continuation_repository = yield* ThreadContinuationRepository;
		const suspend_monitor = yield* HostSuspendMonitor;
		const host_resumes = yield* suspend_monitor.Subscribe;
		const service_scope = yield* Scope.make();
		const live_runs = yield* Ref.make(new Map<string, LiveRun>());
		const dispatch_state = yield* Ref.make<DispatchState>("idle");
		const recovery_started = yield* Ref.make(false);
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
				// Pre-provider validation can fail before the ordinary running transition.
				// Settling it first keeps the canonical run lifecycle from remaining queued.
				yield* repository
					.MarkRunStarted(work.run_id)
					.pipe(Effect.catchCause(() => Effect.succeed([])));
				yield* continuation_repository
					.FailLaunch(work.run_id, `engine_start_${failure.kind}`)
					.pipe(Effect.catchCause(() => Effect.void));
				yield* repository
					.RecordObservation({
						_tag: "process_diagnostic",
						artisan_run_id: work.run_id,
						...(failure.error_ref === undefined
							? {}
							: { error_ref: failure.error_ref }),
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
				yield* Effect.suspend(() => WakeDispatcher);

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

		const observation_persistence = MakeObservationPersistence({
			continuation_repository,
			graph_repository,
			repository,
		});

		const ObserveRun = (work: Pick<PendingWork, "run_id">, live: LiveRun) =>
			live.run.Events.pipe(
				/**
				 * Streaming engines emit per-token deltas far faster than one
				 * transaction per observation can commit. A small window keeps the
				 * write path ahead of the notification stream while staying
				 * invisible next to render cadence.
				 */
				Stream.groupedWithin(256, "100 millis"),
				Stream.runForEach((observations) =>
					observation_persistence.PersistBatch(work, observations.map(PublicObservation)),
				),
			).pipe(
				Effect.andThen(live.run.Closed),
				Effect.asVoid,
				Effect.catchCause((cause) =>
					Effect.sync(() => {
						console.error("Artisan continuation finalization failed", {
							failure_kind: Cause.hasInterruptsOnly(cause)
								? "interrupted"
								: "persistence",
							run_id: work.run_id,
						});
					}),
				),
				Effect.ensuring(
					Effect.gen(function* () {
						yield* Deferred.succeed(live.done, undefined);
						yield* RemoveLiveRun(work.run_id, live.scope);
						yield* Effect.suspend(() => WakeDispatcher);
					}),
				),
			);

		const StartClaimedRun = (work: PendingWork) =>
			Effect.gen(function* () {
				if (work.payload.type !== "thread.send_message") {
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

				const policy = yield* repository.GetSessionPolicy(work.thread_id);
				if (
					!IsSessionPolicyEngine(policy, work.engine_id) &&
					engine.Descriptor.transport !== "test"
				) {
					yield* MarkStartFailure(work, {
						kind: "configuration",
						message:
							"Engine startup failed because the thread policy targets another engine.",
					});

					return;
				}
				const run_metadata = MakeSessionPolicyRunMetadata(policy);
				const run_model = SessionPolicyResolvedModel(policy);
				const prepared_exit = yield* continuation
					.Prepare({
						command_id: work.command_id,
						...(run_metadata.model === undefined
							? {}
							: { target_model: run_metadata.model }),
						...(run_model === undefined ? {} : { target_model_id: run_model }),
						target_run_id: work.run_id,
					})
					.pipe(Effect.exit);

				if (
					Exit.isFailure(prepared_exit) ||
					prepared_exit.value.launch_state !== "prepared"
				) {
					yield* MarkStartFailure(work, {
						...(Exit.isFailure(prepared_exit)
							? { diagnostic: Cause.pretty(prepared_exit.cause) }
							: {}),
						kind: "configuration",
						message:
							"Engine startup failed while preparing durable continuation state.",
					});

					return;
				}
				const prepared = prepared_exit.value;
				const opening = yield* continuation_repository
					.MarkOpening(work.run_id)
					.pipe(Effect.exit);
				if (Exit.isFailure(opening)) {
					yield* MarkStartFailure(work, {
						diagnostic: Cause.pretty(opening.cause),
						kind: "configuration",
						message: "Engine startup failed while fencing its continuation launch.",
					});

					return;
				}
				const started = yield* repository.MarkRunStarted(work.run_id);
				if (started.length === 0) {
					yield* MarkStartFailure(work, {
						kind: "configuration",
						message:
							"Engine startup was rejected because the queued run was no longer startable.",
					});

					return;
				}

				const run_scope = yield* Scope.make();
				const resolved_product_instructions = yield* product_instructions.Resolve;
				const open_input = MakeEngineOpenInput(
					work as PendingWork & {
						readonly payload: AuthoritativeThreadSendMessageCommand;
					},
					prepared,
					run_metadata,
					Option.isSome(resolved_guidance.value)
						? resolved_guidance.value.value
						: undefined,
				);
				const open_exit = yield* engine
					.Open({ ...open_input, product_instructions: resolved_product_instructions })
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
						error_ref: { artisan_code: artisan_error_codes.engine_start_timeout },
						kind: "timeout",
						message: `Engine startup timed out after ${engine_open_timeout_ms / 1_000} seconds.`,
					});

					return;
				}
				const opened_run = run.value;

				const bound = yield* continuation_repository
					.BindTarget({
						command_id: work.command_id,
						...(run_model === undefined ? {} : { model_id: run_model }),
						native_thread_id: opened_run.native_thread_id,
						resume_token: opened_run.resume_token,
						target_run_id: work.run_id,
					})
					.pipe(Effect.exit);

				if (Exit.isFailure(bound)) {
					yield* Scope.close(run_scope, Exit.succeed(undefined));
					yield* MarkStartFailure(work, {
						diagnostic: Cause.pretty(bound.cause),
						kind: "configuration",
						message:
							"Engine startup failed while atomically binding its native session.",
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
				yield* Effect.forkIn(ObserveRun(work, live), service_scope);
			});
		const StartRunUnfenced = (work: PendingWork) =>
			Effect.gen(function* () {
				if ((yield* Ref.get(live_runs)).has(work.run_id)) {
					return;
				}

				const ready = yield* continuation_repository
					.IsDispatchReady(work.run_id)
					.pipe(Effect.catchCause(() => Effect.succeed(false)));
				if (!ready) {
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
				const resolved_product_instructions = yield* product_instructions.Resolve;
				const run = yield* engine
					.Open({
						_tag: "resume",
						artisan_run_id: work.run_id,
						...(Option.isSome(resolved_guidance.value)
							? { global_guidance: resolved_guidance.value.value }
							: {}),
						product_instructions: resolved_product_instructions,
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
					.PersistNativeRun(
						work.run_id,
						run.native_thread_id,
						run.resume_token,
						SessionPolicyResolvedModel(policy),
					)
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

						return;
					}

					/**
					 * A cancel aimed at a run this process is not driving is the user
					 * stopping a run that only the durable record still believes in —
					 * its engine died without a terminal event. There is nothing to
					 * deliver to, so the stop itself is the terminal: record the
					 * cancelled outcome durably rather than dropping the command as
					 * undeliverable and leaving the run wedged forever. Recording is
					 * idempotent against runs that already settled — the observation
					 * is dropped once the run left its projectable statuses.
					 */
					const finalized =
						work.payload.type === "run.cancel"
							? yield* repository
									.RecordObservation({
										_tag: "run_terminal",
										artisan_run_id: work.run_id,
										observation_id: `cancel_fallback:${work.command_id}`,
										raw: {
											engine_id: work.engine_id,
											frame: "cancel_without_live_run",
											transport: "backend",
										},
										sequence: 0,
										state: "cancelled",
									})
									.pipe(
										Effect.as(true),
										Effect.catch(() => Effect.succeed(false)),
									)
							: false;

					if (finalized) {
						yield* repository.CompleteOutbox(work.command_id);
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
			/**
			 * Recovery is a one-shot cold-start operation. In particular, a caller
			 * must not mistake an Engine.Open that has not returned yet for an
			 * ownerless launch merely because it is not in `live_runs`.
			 */
			const first_recovery = yield* Ref.modify(recovery_started, (started) =>
				started ? ([false, true] as const) : ([true, true] as const),
			);
			if (!first_recovery) {
				yield* WakeDispatcher;

				return;
			}

			yield* continuation_repository
				.ReconcileStranded()
				.pipe(Effect.mapError((cause) => new OrchestrationFailure({ cause })));
			yield* graph_repository.RecoverObservedSubagents.pipe(
				Effect.mapError((cause) => new OrchestrationFailure({ cause })),
			);
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
					{
						concurrency: "unbounded",
						discard: true,
					},
				);
				yield* Effect.forEach(runs, (live) => Deferred.await(live.done), {
					concurrency: "unbounded",
					discard: true,
				});
			});
		const QuiesceThread = (thread_id: string) =>
			dispatch_fence.Quiesce(thread_id, QuiesceLiveRuns(thread_id));

		/**
		 * A host suspend can silently cost the process the timers that drive
		 * queued work, so a resume re-reads what is pending from the database
		 * rather than trusting whatever this process was holding when the
		 * machine went away. Runs that were already in flight settle on their
		 * own through the engines' inactivity deadlines, which do not count
		 * suspended time against a run.
		 */
		const WatchHostResumes = Effect.gen(function* () {
			while (true) {
				const resume = yield* PubSub.take(host_resumes);

				yield* Effect.logInfo("Re-driving dispatch after a host suspend", {
					suspended_ms: resume.suspended_ms,
				});
				yield* WakeDispatcher;
			}
		});

		yield* Effect.forkIn(WatchHostResumes, service_scope);
		yield* Recover;

		return { Handle, HandleInbound, QuiesceThread, Recover };
	}),
);
