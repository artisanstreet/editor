import { fileURLToPath } from "node:url";

import { NodeCrypto, NodeFileSystem } from "@effect/platform-node-shared";
import { Deferred, Effect, Fiber, FileSystem, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { ExternalWaitDispatcher } from "../../modules/backend/src/external-wait/external-wait-dispatcher";
import { HostedGitMutationCoordinator } from "../../modules/backend/src/git-provider/hosted-git-mutation-coordinator";
import { AgentGraphOrchestrator } from "../../modules/backend/src/orchestration/agent-graph-orchestrator";
import { AgentOrchestrator } from "../../modules/backend/src/orchestration/agent-orchestrator";
import { HostedProjectCloneCoordinator } from "../../modules/backend/src/projects/hosted-project-clone-coordinator";
import { PreviewBrowserLifecycle } from "../../modules/backend/src/preview/preview-browser";
import { TerminalSessionService } from "../../modules/backend/src/terminal/terminal-sessions";
import {
	ToolControlCoordinator,
	ToolControlCoordinatorLive,
} from "../../modules/backend/src/tool-control/tool-control-coordinator";
import { ToolControlRepositoryLive } from "../../modules/backend/src/tool-control/tool-control-repository";
import { ToolExecutionRepositoryLive } from "../../modules/backend/src/tool-control/tool-execution-repository";
import {
	type ToolRegistration,
	make_tool_registry_layer,
} from "../../modules/backend/src/tool-control/tool-registry";
import { WorkspaceGitCheckoutCoordinator } from "../../modules/backend/src/git/workspace-git-checkout-coordinator";
import { WorkspaceGitFetchService } from "../../modules/backend/src/git/workspace-git-fetch-service";
import { WorkspaceGitMutationCoordinator } from "../../modules/backend/src/git/workspace-git-mutation-coordinator";
import { WorkspaceReplaceApprovalCoordinator } from "../../modules/backend/src/workspace/workspace-replace-approval-coordinator";
import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import {
	OrchestrationRuns,
	ThreadErasureClaims,
	Threads,
	ToolInvocations,
	ToolThreadDispatchState,
} from "../../modules/backend/src/persistence/schema";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";
import { ThreadErasure, ThreadErasureLive } from "../../modules/backend/src/threads/thread-erasure";
import {
	ThreadResourceQuiescer,
	ThreadResourceQuiescerLive,
} from "../../modules/backend/src/threads/thread-resource-quiescer";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: Array<string> = [];
const created_at = "2026-07-16T12:00:00.000Z";
const deleted_at = "2026-07-16T12:01:00.000Z";
const target_thread_id = "thread_tool_erasure_target";
const unrelated_thread_id = "thread_tool_erasure_unrelated";

interface DispatchProbe {
	readonly dispatched_threads: Array<string>;
	readonly entered: Deferred.Deferred<void>;
	readonly release: Deferred.Deferred<void>;
	readonly unrelated_entered: Deferred.Deferred<void>;
}

const descriptor = {
	approval_policy: "automatic" as const,
	effect: "read" as const,
	input_schema: {
		properties: { query: { type: "string" } },
		required: ["query"],
		type: "object",
	},
	label: "Read workspace",
	revision: 1,
	source: "artisan" as const,
	summary: "Runs a bounded test read",
	tool_id: "tool.read",
};

const MakeDatabasePath = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({
		prefix: "artisan-thread-erasure-tool-composition-",
	});

	directories.push(directory);

	return `${directory}/artisan.db`;
}).pipe(Effect.provide(NodeFileSystem.layer));

function metadata_layer(instance_id: string) {
	let identifier = 0;

	return Layer.succeed(RuntimeMetadata, {
		instance_id,
		MakeId: (prefix) => Effect.sync(() => `${prefix}_${instance_id}_${++identifier}`),
		Now: Effect.succeed(created_at),
	});
}

function registration(probe: DispatchProbe): ToolRegistration {
	return {
		adapter: {
			input_schema: descriptor.input_schema,
			Invoke: (context) =>
				Effect.gen(function* () {
					probe.dispatched_threads.push(context.thread_id);

					if (
						context.thread_id === target_thread_id &&
						probe.dispatched_threads.filter(
							(thread_id) => thread_id === target_thread_id,
						).length === 1
					) {
						yield* Deferred.succeed(probe.entered, undefined);
						yield* Deferred.await(probe.release);
					}

					if (context.thread_id === unrelated_thread_id) {
						yield* Deferred.succeed(probe.unrelated_entered, undefined);
					}

					return { ok: true };
				}),
		},
		descriptor,
		IsEligible: () => Effect.void,
		recovery_policy: "retry",
	};
}

function inert_quiescence<Service>(service: { readonly key: string }): Layer.Layer<Service> {
	return Layer.succeed(service as never, { QuiesceThread: () => Effect.void } as never);
}

function make_runtime(database_path: string, probe: DispatchProbe, instance_id: string) {
	const registry = make_tool_registry_layer([registration(probe)]);
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		metadata_layer(instance_id),
		JournalNotifierLive,
		registry,
		NodeCrypto.layer,
	);
	const repositories = Layer.merge(ToolControlRepositoryLive, ToolExecutionRepositoryLive);
	const tool_control = ToolControlCoordinatorLive.pipe(
		Layer.provideMerge(repositories),
		Layer.provideMerge(infrastructure),
	);
	const other_resources = Layer.mergeAll(
		inert_quiescence<typeof ExternalWaitDispatcher.Service>(ExternalWaitDispatcher),
		inert_quiescence<typeof AgentGraphOrchestrator.Service>(AgentGraphOrchestrator),
		inert_quiescence<typeof HostedGitMutationCoordinator.Service>(HostedGitMutationCoordinator),
		inert_quiescence<typeof HostedProjectCloneCoordinator.Service>(
			HostedProjectCloneCoordinator,
		),
		inert_quiescence<typeof AgentOrchestrator.Service>(AgentOrchestrator),
		inert_quiescence<typeof PreviewBrowserLifecycle.Service>(PreviewBrowserLifecycle),
		inert_quiescence<typeof TerminalSessionService.Service>(TerminalSessionService),
		inert_quiescence<typeof WorkspaceGitCheckoutCoordinator.Service>(
			WorkspaceGitCheckoutCoordinator,
		),
		inert_quiescence<typeof WorkspaceGitFetchService.Service>(WorkspaceGitFetchService),
		inert_quiescence<typeof WorkspaceGitMutationCoordinator.Service>(
			WorkspaceGitMutationCoordinator,
		),
		inert_quiescence<typeof WorkspaceReplaceApprovalCoordinator.Service>(
			WorkspaceReplaceApprovalCoordinator,
		),
	);
	const resource_quiescer = ThreadResourceQuiescerLive.pipe(
		Layer.provide(Layer.merge(other_resources, tool_control) as never),
	) as Layer.Layer<ThreadResourceQuiescer>;
	const erasure = ThreadErasureLive.pipe(
		Layer.provideMerge(resource_quiescer),
		Layer.provideMerge(infrastructure),
	);

	return ManagedRuntime.make(Layer.merge(tool_control, erasure));
}

const InstallToolThreadDispatchState = Effect.gen(function* () {
	const database = yield* Database;

	yield* database.client.run(`
		CREATE TABLE IF NOT EXISTS tool_thread_dispatch_state (
			thread_id text PRIMARY KEY REFERENCES threads(thread_id) ON DELETE CASCADE,
			admission_version integer NOT NULL DEFAULT 0,
			quiesced_at text,
			CONSTRAINT tool_thread_dispatch_state_admission_version_check
				CHECK (admission_version >= 0),
			CONSTRAINT tool_thread_dispatch_state_quiesced_at_check
				CHECK (
					quiesced_at IS NULL OR (
						strftime('%Y-%m-%dT%H:%M:%fZ', quiesced_at) IS quiesced_at
						AND substr(quiesced_at, 12, 2) BETWEEN '00' AND '23'
					)
				)
		)
	`);
});

const SeedThread = (thread_id: string) =>
	Effect.gen(function* () {
		const database = yield* Database;

		yield* InstallToolThreadDispatchState;
		yield* database.client.insert(Threads).values({
			created_at,
			last_activity_at: created_at,
			thread_id,
			title: thread_id,
			title_source: "initial",
			updated_at: created_at,
		});
		yield* database.client.insert(OrchestrationRuns).values({
			agent_id: `agent_${thread_id}`,
			created_at,
			engine_id: "codex",
			run_id: `run_${thread_id}`,
			status: "running",
			thread_id,
			updated_at: created_at,
			working_directory: "C:/artisan",
		});
	});

function request(thread_id: string, request_id: string) {
	return {
		arguments: { query: request_id },
		context: {
			agent_id: `agent_${thread_id}`,
			run_id: `run_${thread_id}`,
			thread_id,
		},
		request_id,
		tool: { revision: 1, tool_id: descriptor.tool_id },
	};
}

afterEach(async () => {
	for (const directory of directories.splice(0)) {
		await ManagedRuntime.make(NodeFileSystem.layer).runPromise(
			FileSystem.FileSystem.pipe(
				Effect.flatMap((file_system) => file_system.remove(directory, { recursive: true })),
			),
		);
	}
});

describe("Thread erasure Tool Control production composition", () => {
	it("drains remote target dispatch, permanently fences queued target work, and preserves unrelated dispatch", async () => {
		const entered = await Effect.runPromise(Deferred.make<void>());
		const release = await Effect.runPromise(Deferred.make<void>());
		const unrelated_entered = await Effect.runPromise(Deferred.make<void>());
		const probe = { dispatched_threads: [], entered, release, unrelated_entered };
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const first = make_runtime(database_path, probe, "tool_erasure_first");
		const second = make_runtime(database_path, probe, "tool_erasure_second");

		try {
			await first.runPromise(
				Effect.gen(function* () {
					yield* SeedThread(target_thread_id);
					yield* SeedThread(unrelated_thread_id);
				}),
			);
			const active = await first.runPromise(
				ToolControlCoordinator.pipe(
					Effect.flatMap((coordinator) =>
						coordinator.Invoke(request(target_thread_id, "request_target_active")),
					),
				),
			);

			await Effect.runPromise(Deferred.await(entered).pipe(Effect.timeout("3 seconds")));

			const result = await second.runPromise(
				Effect.gen(function* () {
					const coordinator = yield* ToolControlCoordinator;
					const database = yield* Database;
					const erasure = yield* ThreadErasure;
					const queued = yield* coordinator.Invoke(
						request(target_thread_id, "request_target_queued"),
					);

					yield* database.client.insert(ThreadErasureClaims).values({
						claimed_at: deleted_at,
						thread_id: target_thread_id,
					});

					const erasure_fiber = yield* erasure
						.ResumeClaimed(deleted_at)
						.pipe(Effect.forkChild({ startImmediately: true }));

					for (let attempt = 0; attempt < 300; attempt += 1) {
						const dispatch_states = yield* database.client
							.select()
							.from(ToolThreadDispatchState);
						const dispatch_state = dispatch_states.find(
							(state) => state.thread_id === target_thread_id,
						);

						if (dispatch_state !== undefined && dispatch_state.quiesced_at !== null) {
							break;
						}

						if (attempt === 299) {
							return yield* Effect.die(
								"Durable Tool Control fence was not installed",
							);
						}

						yield* Effect.sleep("10 millis");
					}

					const unrelated = yield* coordinator.Invoke(
						request(unrelated_thread_id, "request_unrelated"),
					);

					yield* Deferred.await(unrelated_entered).pipe(
						Effect.timeout("3 seconds"),
						Effect.mapError(
							(cause) => new Error("Unrelated dispatch did not enter", { cause }),
						),
					);
					yield* coordinator.AwaitIdle;

					const invocations_before_release = yield* database.client
						.select()
						.from(ToolInvocations);
					const completed_before_release = erasure_fiber.pollUnsafe() !== undefined;
					const claims_during_dispatch = yield* database.client
						.select()
						.from(ThreadErasureClaims);
					const unrelated_before_release = invocations_before_release.find(
						(invocation) => invocation.thread_id === unrelated_thread_id,
					)?.state;

					yield* Deferred.succeed(release, undefined);

					const erased = yield* Fiber.join(erasure_fiber).pipe(
						Effect.timeout("3 seconds"),
						Effect.mapError((cause) => new Error("Erasure did not drain", { cause })),
					);
					const fenced_admission = yield* coordinator
						.Invoke(request(target_thread_id, "request_target_after_quiescence"))
						.pipe(Effect.flip);

					const invocations = yield* Effect.gen(function* () {
						let rows = yield* database.client.select().from(ToolInvocations);

						for (let attempt = 0; attempt < 100; attempt += 1) {
							const target_states = rows
								.filter((invocation) => invocation.thread_id === target_thread_id)
								.map((invocation) => invocation.state);
							const unrelated_state = rows.find(
								(invocation) => invocation.thread_id === unrelated_thread_id,
							)?.state;

							if (
								target_states.includes("running") &&
								target_states.includes("pending") &&
								unrelated_state === "completed"
							) {
								return rows;
							}

							yield* Effect.sleep("10 millis");
							rows = yield* database.client.select().from(ToolInvocations);
						}

						return rows;
					});
					const claims_after = yield* database.client.select().from(ThreadErasureClaims);
					const target_after_quiescence = invocations.filter(
						(invocation) => invocation.thread_id === target_thread_id,
					);
					const dispatch_states = yield* database.client
						.select()
						.from(ToolThreadDispatchState);
					const dispatch_state = dispatch_states.find(
						(state) => state.thread_id === target_thread_id,
					);

					return {
						claims_after,
						claims_during_dispatch,
						completed_before_release,
						dispatch_state,
						erased,
						fenced_admission,
						invocations,
						queued,
						target_after_quiescence,
						unrelated,
						unrelated_before_release,
					};
				}),
			);

			expect(active.outcome).toBe("pending");
			expect(result.queued.outcome).toBe("pending");
			expect(result.completed_before_release).toBe(false);
			expect(result.claims_during_dispatch).toMatchObject([{ thread_id: target_thread_id }]);
			expect(result.unrelated_before_release).toBe("completed");
			expect(result.erased).toEqual([]);
			expect(result.claims_after).toEqual([]);
			expect(result.dispatch_state).toMatchObject({
				quiesced_at: created_at,
				thread_id: target_thread_id,
			});
			expect(result.fenced_admission).toMatchObject({
				_tag: "ToolControlUnavailable",
				reason: "erased",
			});
			expect(result.target_after_quiescence).toHaveLength(2);
			expect(result.target_after_quiescence.map((row) => row.state)).toEqual(
				expect.arrayContaining(["running", "pending"]),
			);
			expect(result.unrelated.outcome).toBe("pending");
			expect(probe.dispatched_threads).toEqual([target_thread_id, unrelated_thread_id]);
			expect(
				result.invocations.find(
					(invocation) => invocation.thread_id === unrelated_thread_id,
				)?.state,
			).toBe("completed");
		} finally {
			await Effect.runPromise(Deferred.succeed(release, undefined));
			await first.dispose();
			await second.dispose();
		}
	});
});
