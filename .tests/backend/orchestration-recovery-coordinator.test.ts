import { Context, Deferred, Effect, Exit, Layer, PubSub, Scope } from "effect";
import { describe, expect, it } from "vitest";

import {
	AgentGraphOrchestrator,
	AgentGraphOrchestratorLive,
	AgentOrchestrator,
	OrchestrationRecoveryCoordinatorLive,
	OrchestrationRecoveryGate,
	OrchestrationRecoveryGateLive,
} from "@artisan/backend";
import { EngineRegistry } from "@artisan/engines";
import { GlobalGuidanceService } from "../../modules/backend/src/guidance/service";
import { HostSuspendMonitor } from "../../modules/backend/src/host/suspend-monitor";
import { AgentOrchestratorLive } from "../../modules/backend/src/orchestration/agent-orchestrator";
import { AgentGraphRepository } from "../../modules/backend/src/orchestration/agent-graph-repository";
import { IntakePolicy } from "../../modules/backend/src/orchestration/intake-policy";
import { ProductInstructions } from "../../modules/backend/src/orchestration/product-instructions";
import { OrchestrationRepository } from "../../modules/backend/src/persistence/orchestration/repository";
import { ThreadContinuationRepository } from "../../modules/backend/src/persistence/thread-continuation/repository";
import { UsageInterruptionService } from "../../modules/backend/src/persistence/usage-interruption/service";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/metadata";
import { ThreadContinuationService } from "../../modules/backend/src/orchestration/thread-continuation-service";

const MakeCoordinator = (input: {
	readonly graph_recover: Effect.Effect<void, unknown>;
	readonly graph_start_dispatching?: Effect.Effect<void>;
	readonly root_recover: Effect.Effect<void, unknown>;
	readonly root_start_dispatching?: Effect.Effect<void>;
}) =>
	Layer.mergeAll(
		OrchestrationRecoveryGateLive,
		OrchestrationRecoveryCoordinatorLive.pipe(
			Layer.provideMerge(
				Layer.succeed(AgentOrchestrator, {
					Recover: input.root_recover,
					StartDispatching: input.root_start_dispatching ?? Effect.void,
				} as never),
			),
			Layer.provideMerge(
				Layer.succeed(AgentGraphOrchestrator, {
					Recover: input.graph_recover,
					StartDispatching: input.graph_start_dispatching ?? Effect.void,
				} as never),
			),
			Layer.provideMerge(OrchestrationRecoveryGateLive),
		),
	);

describe("orchestration recovery coordinator", () => {
	it("holds actual root commands outside their repository until recovery succeeds", async () => {
		let accepts = 0;
		const resumes = await Effect.runPromise(PubSub.unbounded<never>());
		const recovery_gate = OrchestrationRecoveryGateLive;
		const root_layer = Layer.mergeAll(
			recovery_gate,
			AgentOrchestratorLive.pipe(
				Layer.provideMerge(recovery_gate),
				Layer.provideMerge(Layer.succeed(EngineRegistry, {} as never)),
				Layer.provideMerge(Layer.succeed(GlobalGuidanceService, {} as never)),
				Layer.provideMerge(Layer.succeed(ProductInstructions, {} as never)),
				Layer.provideMerge(
					Layer.succeed(IntakePolicy, {
						Assess: () =>
							Effect.succeed({
								assumptions: [],
								resolution: "proceed" as const,
								risk: "low" as const,
							}),
					} as never),
				),
				Layer.provideMerge(
					Layer.succeed(OrchestrationRepository, {
						Accept: () => Effect.sync(() => ({ run_id: `${++accepts}` }) as never),
						GetAutoSteer: () => Effect.succeed(false),
						GetSessionPolicy: () => Effect.succeed({ strict_clarification: false }),
					} as never),
				),
				Layer.provideMerge(Layer.succeed(AgentGraphRepository, {} as never)),
				Layer.provideMerge(Layer.succeed(ThreadContinuationService, {} as never)),
				Layer.provideMerge(Layer.succeed(ThreadContinuationRepository, {} as never)),
				Layer.provideMerge(
					Layer.succeed(UsageInterruptionService, {
						CancelSuperseded: () => Effect.void,
					} as never),
				),
				Layer.provideMerge(
					Layer.succeed(HostSuspendMonitor, {
						Subscribe: PubSub.subscribe(resumes),
					} as never),
				),
			),
		);
		const scope = await Effect.runPromise(Scope.make());
		const context = await Effect.runPromise(Layer.buildWithScope(root_layer as never, scope));
		const root = Context.get(context, AgentOrchestrator);
		const gate = Context.get(context, OrchestrationRecoveryGate);
		const command = Effect.runPromise(
			Effect.exit(
				root.Handle({
					kind: "command",
					message_id: "command",
					origin: "frontend",
					payload: { engine_id: "engine", text: "hello", type: "thread.send_message" },
					protocol_version: 1,
					schema_version: 1,
					sent_at: "2026-08-15T00:00:00.000Z",
					thread_id: "thread",
				} as never),
			),
		);

		await new Promise((resolve) => setTimeout(resolve, 10));
		expect(accepts).toBe(0);
		await Effect.runPromise(gate.Complete(Exit.void));
		await command;
		expect(accepts).toBe(1);
		await Effect.runPromise(Scope.close(scope, Exit.void));
	});

	it("holds actual graph commands and reads outside their repository until recovery succeeds", async () => {
		let graph_reads = 0;
		let graph_commands = 0;
		const recovery_gate = OrchestrationRecoveryGateLive;
		const graph_layer = Layer.mergeAll(
			recovery_gate,
			AgentGraphOrchestratorLive.pipe(
				Layer.provideMerge(recovery_gate),
				Layer.provideMerge(Layer.succeed(EngineRegistry, {} as never)),
				Layer.provideMerge(Layer.succeed(GlobalGuidanceService, {} as never)),
				Layer.provideMerge(Layer.succeed(ProductInstructions, {} as never)),
				Layer.provideMerge(
					Layer.succeed(RuntimeMetadata, { instance_id: "instance" } as never),
				),
				Layer.provideMerge(Layer.succeed(OrchestrationRepository, {} as never)),
				Layer.provideMerge(
					Layer.succeed(AgentGraphRepository, {
						GetGraph: () => Effect.sync(() => ++graph_reads as never),
						StartGroup: () => Effect.sync(() => ++graph_commands as never),
					} as never),
				),
			),
		);
		const scope = await Effect.runPromise(Scope.make());
		const context = await Effect.runPromise(Layer.buildWithScope(graph_layer, scope));
		const graph = Context.get(context, AgentGraphOrchestrator);
		const gate = Context.get(context, OrchestrationRecoveryGate);
		const read = Effect.runPromise(Effect.exit(graph.GetGraph("group")));
		const command = Effect.runPromise(
			Effect.exit(
				graph.Handle({
					kind: "command",
					message_id: "command",
					origin: "frontend",
					payload: {
						assignments: [],
						max_concurrency: 1,
						type: "orchestration.group.start",
					},
					protocol_version: 1,
					schema_version: 1,
					sent_at: "2026-08-15T00:00:00.000Z",
					thread_id: "thread",
				} as never),
			),
		);

		await new Promise((resolve) => setTimeout(resolve, 10));
		expect(graph_reads).toBe(0);
		expect(graph_commands).toBe(0);
		await Effect.runPromise(gate.Complete(Exit.void));
		await read;
		await command;
		expect(graph_reads).toBe(1);
		expect(graph_commands).toBe(1);
		await Effect.runPromise(Scope.close(scope, Exit.void));
	});

	it("constructs immediately, gates waiters, and runs root recovery before graph recovery", async () => {
		const root_started = await Effect.runPromise(Deferred.make<void>());
		const release_root = await Effect.runPromise(Deferred.make<void>());
		const recovery_published = await Effect.runPromise(Deferred.make<void>());
		const native_resume_admitted = await Effect.runPromise(Deferred.make<void>());
		const order: Array<string> = [];
		const layer = MakeCoordinator({
			graph_recover: Effect.sync(() => order.push("graph")),
			graph_start_dispatching: Effect.sync(() => order.push("graph-dispatch")),
			root_recover: Effect.gen(function* () {
				order.push("root");
				yield* Deferred.succeed(root_started, undefined);
				yield* Deferred.await(release_root);
			}),
			root_start_dispatching: Deferred.await(recovery_published).pipe(
				Effect.andThen(
					Effect.sync(() => {
						order.push("native-resume");
					}),
				),
				Effect.andThen(Deferred.succeed(native_resume_admitted, undefined)),
			),
		});

		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const context = yield* Layer.build(layer);
					const gate = Context.get(context, OrchestrationRecoveryGate);
					const publish = Effect.runPromise(
						gate.Await.pipe(
							Effect.andThen(Deferred.succeed(recovery_published, undefined)),
						),
					);

					yield* Deferred.await(root_started);
					expect(order).toEqual(["root"]);
					expect(
						(yield* gate.Await.pipe(Effect.timeout("10 millis"), Effect.option))._tag,
					).toBe("None");

					yield* Deferred.succeed(release_root, undefined);
					yield* gate.Await;
					yield* Deferred.await(native_resume_admitted);
					yield* Effect.promise(() => publish);
					expect(order).toEqual(["root", "graph", "native-resume", "graph-dispatch"]);
				}),
			),
		);
	});

	it("settles failed recovery waiters and scope shutdown without waiting for recovery", async () => {
		const entered = await Effect.runPromise(Deferred.make<void>());
		const layer = MakeCoordinator({
			graph_recover: Effect.void,
			root_recover: Deferred.succeed(entered, undefined).pipe(
				Effect.andThen(Effect.fail(new Error("recovery failed"))),
			),
		});

		const scope = await Effect.runPromise(Scope.make());
		const context = await Effect.runPromise(Layer.buildWithScope(layer, scope));
		const gate = Context.get(context, OrchestrationRecoveryGate);

		await Effect.runPromise(Deferred.await(entered));
		const recovery = await Effect.runPromise(Effect.exit(gate.Await));
		expect(Exit.isFailure(recovery)).toBe(true);
		await Effect.runPromise(Scope.close(scope, Exit.void));
	});

	it("settles a stalled recovery waiter when its owning scope is interrupted", async () => {
		const entered = await Effect.runPromise(Deferred.make<void>());
		const layer = MakeCoordinator({
			graph_recover: Effect.void,
			root_recover: Deferred.succeed(entered, undefined).pipe(Effect.andThen(Effect.never)),
		});

		const scope = await Effect.runPromise(Scope.make());
		const context = await Effect.runPromise(Layer.buildWithScope(layer, scope));
		const gate = Context.get(context, OrchestrationRecoveryGate);
		const waiter = Effect.runPromise(Effect.exit(gate.Await));

		await Effect.runPromise(Deferred.await(entered));
		await Effect.runPromise(Scope.close(scope, Exit.void));
		expect(Exit.isFailure(await waiter)).toBe(true);
	});
});
