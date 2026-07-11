import { Context, Deferred, Effect, Exit, Layer, Ref, Scope, Stream } from "effect";

import { EngineRegistry, type EngineCommand, type EngineRun } from "@artisan/engines";
import type { CommandEnvelope, OrchestrationGraph } from "@artisan/protocol";

import { RuntimeMetadata } from "../runtime/runtime-metadata";
import { MakeThreadDispatchFence } from "../threads/internal/thread-dispatch-fence";
import {
	AgentGraphRepository,
	type AcceptedAgentGraphCommand,
	type AgentGraphCommand,
	type AgentGraphControlClaim,
	type AgentGraphError,
	AgentGraphInvalid,
	type PendingAgentRun,
} from "./agent-graph-repository";

interface LiveAgentRun {
	readonly assignment_id: string;
	readonly done: Deferred.Deferred<void>;
	readonly engine_id: string;
	readonly group_id: string;
	readonly run: EngineRun;
	readonly scope: Scope.Closeable;
	readonly thread_id: string;
}

type DispatchState = "idle" | "pending" | "running";

/** Coordinates independent Engine runs for durable multi-agent graphs. */
export class AgentGraphOrchestrator extends Context.Service<
	AgentGraphOrchestrator,
	{
		readonly Handle: (
			command: CommandEnvelope,
		) => Effect.Effect<AcceptedAgentGraphCommand, AgentGraphError>;
		readonly GetGraph: (group_id: string) => Effect.Effect<OrchestrationGraph, AgentGraphError>;
		readonly Recover: Effect.Effect<void, AgentGraphError>;
		readonly QuiesceThread: (thread_id: string) => Effect.Effect<void>;
	}
>()("Artisan/AgentGraphOrchestrator") {}

function command_failure_message(error: unknown) {
	return error instanceof Error ? error.message : "The engine rejected the assignment command.";
}

export const AgentGraphOrchestratorLive = Layer.effect(
	AgentGraphOrchestrator,
	Effect.gen(function* () {
		const engines = yield* EngineRegistry;
		const metadata = yield* RuntimeMetadata;
		const repository = yield* AgentGraphRepository;
		const service_scope = yield* Scope.make();
		const live_runs = yield* Ref.make(new Map<string, LiveAgentRun>());
		const dispatch_state = yield* Ref.make<DispatchState>("idle");
		const dispatch_fence = yield* MakeThreadDispatchFence;

		const remove_live = (run_id: string, run_scope: Scope.Closeable) =>
			Effect.gen(function* () {
				const removed = yield* Ref.modify(live_runs, (runs) => {
					const live = runs.get(run_id);

					if (live?.scope !== run_scope) {
						return [false, runs] as const;
					}

					const next = new Map(runs);

					next.delete(run_id);

					return [true, next] as const;
				});

				if (removed) {
					yield* Scope.close(run_scope, Exit.void);
				}
			});

		function wake_dispatcher(): Effect.Effect<void> {
			return Effect.gen(function* () {
				const start = yield* Ref.modify(dispatch_state, (state) =>
					state === "idle" ? ([true, "running"] as const) : ([false, "pending"] as const),
				);

				if (start) {
					yield* Effect.forkIn(dispatch_loop(), service_scope);
				}
			});
		}

		const observe_run = (work: PendingAgentRun, live: LiveAgentRun) =>
			Stream.runForEach(live.run.Events, (observation) =>
				repository.RecordObservation(observation).pipe(Effect.andThen(wake_dispatcher())),
			).pipe(
				Effect.catch(() => Effect.void),
				Effect.andThen(live.run.Closed),
				Effect.flatMap((state) => repository.RecordClosed(work.run_id, state)),
				Effect.andThen(wake_dispatcher()),
				Effect.catch(() => Effect.void),
				Effect.ensuring(
					Effect.gen(function* () {
						yield* Deferred.succeed(live.done, undefined);
						yield* remove_live(work.run_id, live.scope);
					}),
				),
			);

		const register_live = (work: PendingAgentRun, run: EngineRun, run_scope: Scope.Closeable) =>
			Effect.gen(function* () {
				const done = yield* Deferred.make<void>();
				const live = {
					assignment_id: work.assignment_id,
					done,
					engine_id: work.engine_id,
					group_id: work.group_id,
					run,
					scope: run_scope,
					thread_id: work.thread_id,
				} satisfies LiveAgentRun;

				yield* Ref.update(live_runs, (runs) => new Map(runs).set(work.run_id, live));
				yield* Effect.forkIn(observe_run(work, live), service_scope);
			});

		const fail_start = (work: PendingAgentRun, failure: string) =>
			repository
				.FailRunStart(work.run_id, metadata.instance_id, failure)
				.pipe(Effect.andThen(wake_dispatcher()));

		const start_run_unfenced = (work: PendingAgentRun) =>
			Effect.gen(function* () {
				if ((yield* Ref.get(live_runs)).has(work.run_id)) {
					return;
				}

				const claimed = yield* repository.ClaimRun(work.run_id, metadata.instance_id);

				if (!claimed) {
					return;
				}

				const engine = yield* engines.Get(work.engine_id).pipe(Effect.option);

				if (engine._tag === "None") {
					yield* fail_start(work, `Engine ${work.engine_id} is unavailable.`);

					return;
				}

				const run_scope = yield* Scope.make();
				let transferred = false;

				return yield* Effect.gen(function* () {
					const opened = yield* engine.value
						.Open({
							_tag: "start",
							artisan_run_id: work.run_id,
							initial_text: [
								work.instructions,
								`Expected result: ${work.expected_result}`,
								`Summary contract: ${work.summary_contract}`,
							].join("\n\n"),
							model: work.profile,
							permission_policy: {
								approval: work.permission_policy.approval,
								network_access: work.permission_policy.network_access,
								write_access: work.permission_policy.write_access,
							},
							working_directory: work.workspace.working_directory,
						})
						.pipe(Scope.provide(run_scope), Effect.exit);

					if (Exit.isFailure(opened)) {
						yield* fail_start(work, "The engine could not start this assignment.");

						return;
					}

					const run = opened.value;

					return yield* Effect.gen(function* () {
						yield* repository.ActivateRun(
							work.run_id,
							metadata.instance_id,
							run.native_thread_id,
							run.resume_token,
						);
						yield* register_live(work, run, run_scope);
						transferred = true;
					}).pipe(Effect.uninterruptible);
				}).pipe(
					Effect.catch((error) => fail_start(work, command_failure_message(error))),
					Effect.ensuring(
						Effect.suspend(() =>
							transferred ? Effect.void : Scope.close(run_scope, Exit.void),
						),
					),
				);
			});
		const start_run = (work: PendingAgentRun) =>
			dispatch_fence.Run(work.thread_id, start_run_unfenced(work)).pipe(Effect.asVoid);

		const select_dispatchable = (
			pending: ReadonlyArray<PendingAgentRun>,
			live: ReadonlyMap<string, LiveAgentRun>,
		) => {
			const active_by_group = new Map<string, number>();
			const selected: Array<PendingAgentRun> = [];

			for (const current of live.values()) {
				active_by_group.set(
					current.group_id,
					(active_by_group.get(current.group_id) ?? 0) + 1,
				);
			}

			for (const work of pending) {
				if (selected.length >= 16) {
					break;
				}

				const active = active_by_group.get(work.group_id) ?? 0;

				if (active >= work.max_concurrency) {
					continue;
				}

				selected.push(work);
				active_by_group.set(work.group_id, active + 1);
			}

			return selected;
		};

		const dispatch_pending = Effect.gen(function* () {
			const pending = yield* repository.GetPendingRuns();
			const live = yield* Ref.get(live_runs);
			const selected = select_dispatchable(pending, live);

			yield* Effect.forEach(selected, start_run, {
				concurrency: "unbounded",
				discard: true,
			});
		});

		function dispatch_loop(): Effect.Effect<void> {
			return Effect.gen(function* () {
				do {
					yield* dispatch_pending;
				} while (
					yield* Ref.modify(dispatch_state, (state) =>
						state === "pending"
							? ([true, "running"] as const)
							: ([false, "idle"] as const),
					)
				);
			}).pipe(Effect.catch(() => Effect.asVoid(Ref.set(dispatch_state, "idle"))));
		}

		const replay_control = (
			claim: AgentGraphControlClaim,
			events: ReadonlyArray<AcceptedAgentGraphCommand["events"][number]>,
		) => {
			const event = events.at(-1);

			return event
				? Effect.succeed({
						events,
						group_id: claim.group_id,
						journal_sequence: event.journal_sequence,
						status: "duplicate" as const,
					})
				: Effect.fail(
						new AgentGraphInvalid({
							message: "A completed graph control command has no correlated event",
						}),
					);
		};

		const handle_control_unfenced = (command: CommandEnvelope) =>
			Effect.gen(function* () {
				const claim = yield* repository.ClaimControl(command);

				if (claim.status === "duplicate") {
					const events = yield* repository.ReadCommandEvents(command.message_id);

					if (claim.command_status === "dispatching") {
						const event = events.at(-1);

						if (event) {
							yield* repository.FinalizeControl(claim, event);

							return yield* replay_control(claim, events);
						}

						return yield* repository.CompleteControl(
							command,
							claim,
							"ambiguous",
							"A previously claimed control command has an ambiguous dispatch result.",
						);
					}

					return yield* replay_control(claim, events);
				}

				if (claim.action === "pause" || claim.action === "resume") {
					return yield* repository.CompleteControl(
						command,
						claim,
						"unsupported",
						"The provider-neutral Engine seam does not expose pause or resume.",
					);
				}

				const live = (yield* Ref.get(live_runs)).get(claim.run_id);

				if (!live || live.assignment_id !== claim.assignment_id) {
					return yield* repository.CompleteControl(
						command,
						claim,
						"ambiguous",
						"The claimed run is no longer owned by this backend instance.",
					);
				}

				const engine = yield* engines.Get(live.engine_id).pipe(Effect.option);

				if (engine._tag === "None") {
					return yield* repository.CompleteControl(
						command,
						claim,
						"rejected",
						"The assignment engine is no longer registered.",
					);
				}

				const capabilities = engine.value.Descriptor.capabilities;
				const payload = command.payload as AgentGraphCommand;
				const command_to_send: EngineCommand | undefined =
					claim.action === "steer" && payload.type === "assignment.steer"
						? capabilities.steer.state === "unsupported"
							? undefined
							: { _tag: "steer", command_id: command.message_id, text: payload.text }
						: claim.action === "stop" && capabilities.cancel.state !== "unsupported"
							? { _tag: "cancel", command_id: command.message_id }
							: claim.action === "stop" && capabilities.close.state !== "unsupported"
								? { _tag: "close", command_id: command.message_id }
								: undefined;

				if (!command_to_send) {
					return yield* repository.CompleteControl(
						command,
						claim,
						"unsupported",
						`The engine does not support ${claim.action}.`,
					);
				}

				const delivered = yield* live.run.Send(command_to_send).pipe(Effect.exit);

				if (Exit.isSuccess(delivered)) {
					return yield* repository.CompleteControl(command, claim, "accepted");
				}

				return yield* repository.CompleteControl(
					command,
					claim,
					"rejected",
					command_failure_message(delivered.cause),
				);
			});
		const handle_control = (command: CommandEnvelope) =>
			Effect.gen(function* () {
				const result = yield* dispatch_fence.Run(
					command.thread_id,
					handle_control_unfenced(command),
				);

				if (result._tag === "None") {
					return yield* new AgentGraphInvalid({
						message: `Thread ${command.thread_id} is quiesced`,
					});
				}

				return result.value;
			});

		const handle = (command: CommandEnvelope) => {
			const payload = command.payload as AgentGraphCommand;

			if (payload.type === "orchestration.group.start") {
				return repository.StartGroup(command).pipe(Effect.tap(() => wake_dispatcher()));
			}

			if (payload.type === "agent_instance.rename") {
				return repository.RenameAgent(command);
			}

			if (payload.type === "assignment.heartbeat") {
				return repository.RecordHeartbeat(command);
			}

			if (payload.type === "assignment.retry") {
				return repository
					.RetryAssignment(command)
					.pipe(Effect.tap(() => wake_dispatcher()));
			}

			return handle_control(command);
		};

		const recover = repository
			.Recover(metadata.instance_id)
			.pipe(Effect.andThen(wake_dispatcher()));
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

				yield* Effect.forEach(runs, (live) => Scope.close(live.scope, Exit.void), {
					concurrency: "unbounded",
					discard: true,
				});
				yield* Effect.forEach(runs, (live) => Deferred.await(live.done), {
					concurrency: "unbounded",
					discard: true,
				});
			});
		const QuiesceThread = (thread_id: string) =>
			dispatch_fence.Quiesce(thread_id, QuiesceLiveRuns(thread_id));
		const shutdown = Effect.gen(function* () {
			const runs = [...(yield* Ref.get(live_runs)).values()];

			yield* Effect.forEach(runs, (live) => Scope.close(live.scope, Exit.void), {
				concurrency: "unbounded",
				discard: true,
			});
			yield* Effect.forEach(runs, (live) => Deferred.await(live.done), {
				concurrency: "unbounded",
				discard: true,
			});
			yield* Scope.close(service_scope, Exit.void);
		});

		yield* Effect.addFinalizer(() => shutdown);
		yield* recover;

		return {
			GetGraph: repository.GetGraph,
			Handle: handle,
			QuiesceThread,
			Recover: recover,
		};
	}),
);
