import {
	Context,
	Data,
	Effect,
	Exit,
	Layer,
	Option,
	Queue,
	Ref,
	Result,
	Schema,
	Scope,
	SubscriptionRef,
} from "effect";

import {
	type DecideApprovalRequest,
	type DecideApprovalResult,
	type InvokeRequest,
	type InvokeResult,
	InvokeResult as InvokeResultSchema,
	type ListEligibleRequest,
	type ListEligibleResult,
	type ToolApprovalQuery,
	type ToolApprovalQueryResult,
	type ToolInvocationProjection,
	type ToolInvocationQuery,
	type ToolInvocationQueryResult,
} from "@artisan/protocol";

import { ToolControlRepository, type ToolControlRepositoryError } from "./tool-control-repository";
import {
	ToolExecutionRepository,
	type ToolExecution,
	type ToolExecutionRepositoryError,
} from "./tool-execution-repository";
import { ToolRegistry, type ToolRegistryError } from "./tool-registry";
import { MakeThreadDispatchFence } from "../threads/internal/thread-dispatch-fence";

/** Reports a source-safe coordinator failure without exposing tool arguments or results. */
export class ToolControlCoordinatorFailure extends Data.TaggedError(
	"ToolControlCoordinatorFailure",
)<{ readonly reason: "invalid_lifecycle" }> {}

/** Represents failures surfaced by the durable Tool Control coordinator. */
export type ToolControlCoordinatorError =
	| ToolControlCoordinatorFailure
	| ToolControlRepositoryError
	| ToolExecutionRepositoryError
	| ToolRegistryError;

/** Coordinates durable tool admission, approval, execution, replay, and crash recovery. */
export class ToolControlCoordinator extends Context.Service<
	ToolControlCoordinator,
	{
		readonly AwaitIdle: Effect.Effect<void>;
		readonly Decide: (
			input: DecideApprovalRequest,
		) => Effect.Effect<DecideApprovalResult, ToolControlCoordinatorError>;
		readonly Invoke: (
			input: InvokeRequest,
		) => Effect.Effect<InvokeResult, ToolControlCoordinatorError>;
		readonly ListEligible: (
			input: ListEligibleRequest,
		) => Effect.Effect<ListEligibleResult, ToolControlRepositoryError | ToolRegistryError>;
		readonly QueryApproval: (
			query: ToolApprovalQuery,
		) => Effect.Effect<ToolApprovalQueryResult, ToolControlRepositoryError>;
		readonly QueryInvocation: (
			query: ToolInvocationQuery,
		) => Effect.Effect<ToolInvocationQueryResult, ToolControlRepositoryError>;
		readonly QuiesceThread: (
			thread_id: string,
		) => Effect.Effect<void, ToolExecutionRepositoryError>;
		readonly Recover: Effect.Effect<void, ToolControlCoordinatorError>;
	}
>()("Artisan/ToolControlCoordinator") {}

const dispatch_worker_count = 4;
const dispatch_queue_capacity = dispatch_worker_count;

interface DispatchState {
	readonly active: number;
	readonly state: "idle" | "pending" | "running";
}

interface DispatchWork {
	readonly invocation_id: string;
	readonly thread_id: string;
	readonly Execute: Effect.Effect<void, ToolControlCoordinatorError>;
}

function dispatch_is_idle(state: DispatchState): boolean {
	return state.active === 0 && state.state === "idle";
}

function invalid_lifecycle() {
	return new ToolControlCoordinatorFailure({ reason: "invalid_lifecycle" });
}

function invoke_result(
	invocation: ToolInvocationProjection,
): Effect.Effect<InvokeResult, ToolControlCoordinatorFailure> {
	if (invocation.state === "completed") {
		return Effect.fail(invalid_lifecycle());
	}

	return Schema.decodeUnknownEffect(InvokeResultSchema, { onExcessProperty: "error" })({
		invocation,
		outcome: invocation.state,
	}).pipe(Effect.mapError(invalid_lifecycle));
}

/** Supplies scoped durable dispatch that serializes wakeups without polling idle work. */
export const ToolControlCoordinatorLive = Layer.effect(
	ToolControlCoordinator,
	Effect.gen(function* () {
		const controls = yield* ToolControlRepository;
		const executions = yield* ToolExecutionRepository;
		const registry = yield* ToolRegistry;
		const dispatch_fence = yield* MakeThreadDispatchFence;
		const dispatch_state = yield* SubscriptionRef.make<DispatchState>({
			active: 0,
			state: "idle",
		});
		const dispatch_queue = yield* Queue.bounded<DispatchWork>(dispatch_queue_capacity);
		const active_threads = yield* Ref.make(new Set<string>());
		const dispatch_loop_active = yield* Ref.make(false);
		const service_scope = yield* Scope.make();

		yield* Effect.addFinalizer(() =>
			Queue.shutdown(dispatch_queue).pipe(
				Effect.andThen(Scope.close(service_scope, Exit.void)),
				Effect.andThen(executions.AbandonOwnedExecutions),
				Effect.ignoreCause,
			),
		);

		const Heartbeat = (execution: ToolExecution) =>
			Effect.forever(
				Effect.sleep("5 seconds").pipe(
					Effect.andThen(
						executions.RenewLease({
							claim_token: execution.claim_token,
							invocation_id: execution.invocation.invocation_id,
						}),
					),
				),
			);
		const WithLease = <A, E, R>(execution: ToolExecution, effect: Effect.Effect<A, E, R>) =>
			Effect.raceFirst(effect, Heartbeat(execution));
		const Execute = (execution: ToolExecution) =>
			Effect.gen(function* () {
				const identity = {
					claim_token: execution.claim_token,
					invocation_id: execution.invocation.invocation_id,
				};

				if (execution.launch_started) {
					return;
				}

				yield* executions.MarkLaunchStarted(identity);
				const outcome = yield* registry
					.Invoke(
						{
							context: execution.invocation.context,
							invocation_id: execution.invocation.invocation_id,
							tool: {
								revision: execution.invocation.tool.revision,
								tool_id: execution.invocation.tool.tool_id,
							},
						},
						execution.arguments,
					)
					.pipe(Effect.result);

				if (Result.isFailure(outcome)) {
					yield* executions.SettleFailed(identity);

					return;
				}

				yield* executions.SettleCompleted(identity, outcome.success);
			});
		const ExecuteWithLease = (execution: ToolExecution) => {
			const identity = {
				claim_token: execution.claim_token,
				invocation_id: execution.invocation.invocation_id,
			};

			return WithLease(execution, Execute(execution)).pipe(
				Effect.onExit((exit) =>
					Exit.isFailure(exit)
						? executions.AbandonExecution(identity).pipe(Effect.ignoreCause)
						: Effect.void,
				),
			);
		};
		const ReleaseThread = (work: DispatchWork) =>
			Effect.gen(function* () {
				yield* Ref.update(active_threads, (current) => {
					const next = new Set(current);

					next.delete(work.thread_id);

					return next;
				});
			});
		const CompleteDispatch = () =>
			Effect.gen(function* () {
				yield* SubscriptionRef.update(dispatch_state, (current) => ({
					...current,
					active: current.active - 1,
				}));
			});
		const FinishDispatch = (work: DispatchWork) =>
			ReleaseThread(work).pipe(Effect.andThen(CompleteDispatch));
		const Schedule = (work: DispatchWork) =>
			Effect.uninterruptibleMask((restore) =>
				Effect.gen(function* () {
					const admitted = yield* Ref.modify(active_threads, (current) => {
						if (current.has(work.thread_id)) {
							return [false, current] as const;
						}

						return [true, new Set(current).add(work.thread_id)] as const;
					});

					if (!admitted) {
						return;
					}

					yield* SubscriptionRef.update(dispatch_state, (current) => ({
						...current,
						active: current.active + 1,
					}));
					const offered = yield* restore(Queue.offer(dispatch_queue, work)).pipe(
						Effect.onExit((exit) =>
							Exit.isFailure(exit) ? FinishDispatch(work) : Effect.void,
						),
					);

					if (!offered) {
						yield* FinishDispatch(work);
					}
				}),
			);
		const DispatchPending = Effect.gen(function* () {
			const pending = yield* executions.ListPending;
			const results = yield* Effect.forEach(
				pending,
				({ invocation_id, thread_id }) =>
					Schedule({
						invocation_id,
						thread_id,
						Execute: executions.ClaimPending(invocation_id).pipe(
							Effect.flatMap((claim) =>
								Option.match(claim, {
									onNone: () => Effect.void,
									onSome: ExecuteWithLease,
								}),
							),
						),
					}).pipe(Effect.exit),
				{ concurrency: dispatch_worker_count },
			);

			return results.some(Exit.isFailure);
		});
		const RecoverRunning = Effect.gen(function* () {
			const running = yield* executions.ListRunning;
			const active = running.filter(({ recovery }) => recovery !== "waiting");
			const results = yield* Effect.forEach(
				active,
				(dispatch) =>
					Schedule({
						invocation_id: dispatch.invocation_id,
						thread_id: dispatch.thread_id,
						Execute: Effect.gen(function* () {
							if (dispatch.recovery === "quarantine") {
								yield* executions.QuarantineInterrupted(dispatch.invocation_id);

								return;
							}

							if (dispatch.recovery === "owned") {
								const execution = yield* executions.ReadExecution(
									dispatch.identity!,
								);

								if (!execution.launch_started) {
									yield* ExecuteWithLease(execution);
								}

								return;
							}

							const recovered = yield* executions.ClaimRecovery(
								dispatch.invocation_id,
							);

							if (Option.isSome(recovered)) {
								yield* ExecuteWithLease(recovered.value);
							}
						}),
					}).pipe(Effect.exit),
				{ concurrency: dispatch_worker_count },
			);

			const awaiting_lease_expiry = running.some(({ recovery }) => recovery === "waiting");

			return results.some(Exit.isFailure) || awaiting_lease_expiry;
		});
		const DispatchWork = Effect.gen(function* () {
			const recovery_retry = yield* RecoverRunning;
			const pending_retry = yield* DispatchPending;

			return recovery_retry || pending_retry;
		});
		const FinalizeDispatchLoop: Effect.Effect<void> = Effect.gen(function* () {
			yield* Ref.set(dispatch_loop_active, false);

			const state = yield* SubscriptionRef.get(dispatch_state);

			if (state.state === "pending") {
				yield* WakeDispatcher;
			}
		});
		const DispatchLoop: Effect.Effect<void> = Effect.gen(function* () {
			while (true) {
				const dispatched = yield* DispatchWork.pipe(Effect.exit);
				const retry = Exit.isFailure(dispatched) || dispatched.value;
				const continue_dispatch = yield* SubscriptionRef.modify(dispatch_state, (state) => {
					const requested = state.state === "pending";
					const keep_running = retry || requested;

					return [
						keep_running,
						{ ...state, state: keep_running ? "running" : "idle" },
					] as const;
				});

				if (!continue_dispatch) {
					return;
				}

				if (retry) {
					yield* Effect.sleep("1 second");
				}
			}
		}).pipe(Effect.ensuring(FinalizeDispatchLoop));
		const RunDispatch = (work: DispatchWork) =>
			dispatch_fence.Run(work.thread_id, work.Execute).pipe(Effect.exit);
		const DispatchWorker = Effect.forever(
			Queue.take(dispatch_queue).pipe(
				Effect.flatMap((work) =>
					Effect.gen(function* () {
						const dispatched = yield* RunDispatch(work);
						const continue_dispatch =
							Exit.isFailure(dispatched) || Option.isSome(dispatched.value);

						if (continue_dispatch) {
							yield* ReleaseThread(work);
							yield* WakeDispatcher;
							yield* CompleteDispatch();

							return;
						}

						yield* FinishDispatch(work);
					}),
				),
			),
		);
		const WakeDispatcher: Effect.Effect<void> = Effect.gen(function* () {
			const start = yield* Ref.modify(dispatch_loop_active, (active) =>
				active ? ([false, active] as const) : ([true, true] as const),
			);

			yield* SubscriptionRef.update(dispatch_state, (state) => ({
				...state,
				state: start ? ("running" as const) : ("pending" as const),
			}));

			if (start) {
				yield* Effect.forkIn(DispatchLoop, service_scope);
			}
		});
		const AwaitIdle = Effect.gen(function* () {
			while (true) {
				const current = yield* SubscriptionRef.get(dispatch_state);

				if (!dispatch_is_idle(current)) {
					yield* Effect.sleep("1 millis");

					continue;
				}

				yield* Effect.sleep("25 millis");

				const settled = yield* SubscriptionRef.get(dispatch_state);

				if (dispatch_is_idle(settled)) {
					return;
				}
			}
		});
		const ListEligible = (input: ListEligibleRequest) =>
			controls
				.AuthorizeContext(input.context)
				.pipe(Effect.andThen(registry.List(input.context)));
		const QueryApproval = controls.QueryApproval;
		const QueryInvocation = controls.QueryInvocation;
		const Invoke = (input: InvokeRequest) =>
			Effect.gen(function* () {
				const prepared = yield* controls.Prepare(input);

				if (prepared.invocation.state === "pending") {
					yield* WakeDispatcher;
				}

				if (prepared.invocation.state !== "completed") {
					return yield* invoke_result(prepared.invocation);
				}

				const completed = yield* executions.ReadCompleted(input);

				if (Option.isNone(completed)) {
					return yield* invalid_lifecycle();
				}

				return yield* Schema.decodeUnknownEffect(InvokeResultSchema, {
					onExcessProperty: "error",
				})({
					invocation: completed.value.invocation,
					outcome: "completed",
					result: completed.value.result,
				}).pipe(Effect.mapError(invalid_lifecycle));
			});
		const Decide = (input: DecideApprovalRequest) =>
			Effect.gen(function* () {
				const decided = yield* controls.Decide(input);

				if (decided.invocation.state === "pending") {
					yield* WakeDispatcher;
				}

				return { approval: decided.approval };
			});
		const Recover = WakeDispatcher;
		const AwaitRemoteQuiescence = (thread_id: string) =>
			Effect.gen(function* () {
				while (yield* executions.ThreadQuiescencePending(thread_id)) {
					yield* Effect.sleep("10 millis");
				}
			});
		const QuiesceThread = (thread_id: string) =>
			executions
				.BeginThreadQuiescence(thread_id)
				.pipe(
					Effect.andThen(dispatch_fence.Quiesce(thread_id, Effect.void)),
					Effect.andThen(AwaitRemoteQuiescence(thread_id)),
				);

		yield* Effect.forEach(
			Array.from({ length: dispatch_worker_count }),
			() =>
				Effect.forkIn(DispatchWorker, service_scope, {
					startImmediately: true,
					uninterruptible: false,
				}),
			{ discard: true },
		);
		yield* Recover;

		return {
			AwaitIdle,
			Decide,
			Invoke,
			ListEligible,
			QueryApproval,
			QueryInvocation,
			QuiesceThread,
			Recover,
		};
	}),
);
