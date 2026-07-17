import { fileURLToPath } from "node:url";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, FileSystem, Layer } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { make_backend_runtime, ProtocolServer } from "@artisan/backend";

import { Database } from "../../modules/backend/src/persistence/database";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";
import { ToolControlCommands } from "../../modules/backend/src/persistence/schema";
import { ToolControlCoordinator } from "../../modules/backend/src/tool-control/tool-control-coordinator";
import {
	make_tool_registry_layer,
	type ToolRegistration,
} from "../../modules/backend/src/tool-control/tool-registry";

import {
	make_transport_test_harness_with_protocol_server,
	wait_for,
} from "./message-channel-harness";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: Array<string> = [];

interface ToolState {
	readonly calls: Record<string, number>;
}

const descriptor = (tool_id: string, approval_policy: "automatic" | "required") => ({
	approval_policy,
	effect: "read" as const,
	input_schema: { type: "object" },
	label: tool_id,
	revision: 1,
	source: "artisan" as const,
	summary: "Runs a deterministic protocol test tool",
	tool_id,
});

function make_metadata_layer() {
	let identifier = 0;

	return Layer.succeed(RuntimeMetadata, {
		instance_id: "tool_control_protocol",
		MakeId: (prefix) => Effect.sync(() => `${prefix}_${++identifier}`),
		Now: Effect.succeed("2026-07-16T12:00:00.000Z"),
	});
}

function registration(
	tool_id: string,
	approval_policy: "automatic" | "required",
	state: ToolState,
): ToolRegistration {
	const current_descriptor = descriptor(tool_id, approval_policy);

	return {
		adapter: {
			input_schema: current_descriptor.input_schema,
			Invoke: () =>
				Effect.sync(() => {
					state.calls[tool_id] = (state.calls[tool_id] ?? 0) + 1;

					return { private_result: "secret-tool-result" };
				}),
		},
		descriptor: current_descriptor,
		IsEligible: () => Effect.void,
		recovery_policy: "retry",
	};
}

const MakeDatabasePath = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({
		prefix: "artisan-tool-control-protocol-",
	});

	directories.push(directory);

	return `${directory}/artisan.db`;
}).pipe(Effect.provide(NodeFileSystem.layer));

const SeedOrdinaryOwnership = Database.pipe(
	Effect.flatMap((database) =>
		database.client
			.run(`
				INSERT INTO threads (thread_id, title, title_source, created_at, updated_at)
				VALUES ('thread_1', 'Tool control', 'initial', '2026-07-16T12:00:00.000Z', '2026-07-16T12:00:00.000Z')
			`)
			.pipe(
				Effect.andThen(
					database.client.run(`
						INSERT INTO orchestration_runs
						(run_id, thread_id, agent_id, engine_id, status, working_directory, created_at, updated_at)
						VALUES ('run_1', 'thread_1', 'agent_1', 'test', 'running', 'C:/artisan', '2026-07-16T12:00:00.000Z', '2026-07-16T12:00:00.000Z')
					`),
				),
			),
	),
);

async function start_stack(
	state: ToolState,
	options: NonNullable<
		Parameters<typeof make_transport_test_harness_with_protocol_server>[1]
	> = {},
) {
	const database_path = await Effect.runPromise(MakeDatabasePath);
	const tool_registry = make_tool_registry_layer([
		registration("tool.automatic", "automatic", state),
		registration("tool.approval", "required", state),
	]);
	const runtime = make_backend_runtime({
		database_path,
		migrations_path,
		runtime_metadata: make_metadata_layer(),
		tool_registry,
	});
	const protocol_server = await runtime.runPromise(ProtocolServer);
	const harness = await make_transport_test_harness_with_protocol_server(
		protocol_server,
		options,
	);

	await runtime.runPromise(SeedOrdinaryOwnership);

	return { harness, runtime };
}

afterEach(async () => {
	const cleanup = directories.splice(0);

	await Effect.runPromise(
		Effect.forEach(cleanup, (directory) =>
			FileSystem.FileSystem.pipe(
				Effect.flatMap((file_system) => file_system.remove(directory, { recursive: true })),
			),
		).pipe(Effect.provide(NodeFileSystem.layer)),
	);
});

describe("ArtisanClient Tool Control with the backend ProtocolServer", () => {
	it("retries the exact invoke envelope after a dropped result and executes once", async () => {
		const state = { calls: {} };
		const { harness, runtime } = await start_stack(state, {
			drop_first_tool_invoke_result: true,
		});

		try {
			const invoked = Effect.runPromise(
				harness.client.InvokeTool({
					arguments: { private_input: "secret-tool-input" },
					context: {
						agent_id: "agent_1",
						run_id: "run_1",
						thread_id: "thread_1",
					},
					request_id: "invoke_reconnect_1",
					tool: { revision: 1, tool_id: "tool.automatic" },
				}),
			);

			await wait_for(() => harness.connector_snapshot().tool_invoke_attempts.length === 2);

			const result = await invoked;
			const snapshot = harness.connector_snapshot();

			expect(result).toMatchObject({
				outcome: "completed",
				result: { private_result: "secret-tool-result" },
			});
			expect(snapshot.connections).toBe(2);
			expect(snapshot.dropped_tool_invoke_results).toBe(1);
			expect(snapshot.tool_invoke_attempts).toHaveLength(2);
			expect(snapshot.tool_invoke_attempts[1]).toEqual(snapshot.tool_invoke_attempts[0]);
			expect(snapshot.tool_invoke_attempts[0]).toMatchObject({
				message_id: "invoke_reconnect_1",
				payload: { request_id: "invoke_reconnect_1" },
			});
			expect(state.calls).toEqual({ "tool.automatic": 1 });
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	});

	it("retries the exact approval decision after a dropped result and decides once", async () => {
		const state = { calls: {} };
		const { harness, runtime } = await start_stack(state, {
			drop_first_tool_approval_decide_result: true,
		});

		try {
			const invoked = await Effect.runPromise(
				harness.client.InvokeTool({
					arguments: { private_input: "secret-tool-input" },
					context: {
						agent_id: "agent_1",
						run_id: "run_1",
						thread_id: "thread_1",
					},
					request_id: "approval_reconnect_invoke_1",
					tool: { revision: 1, tool_id: "tool.approval" },
				}),
			);

			if (invoked.outcome !== "approval_required") {
				throw new Error("Expected a required tool to request approval.");
			}

			const decided = Effect.runPromise(
				harness.client.DecideToolApproval({
					approval_id: invoked.invocation.approval_id,
					decision: "approved",
					decision_id: "decision_reconnect_1",
					thread_id: "thread_1",
				}),
			);

			await wait_for(
				() => harness.connector_snapshot().tool_approval_decide_attempts.length === 2,
			);

			const result = await decided;
			const coordinator = await runtime.runPromise(ToolControlCoordinator);

			await runtime.runPromise(coordinator.AwaitIdle);

			const commands = await runtime.runPromise(
				Database.pipe(
					Effect.flatMap((database) =>
						database.client.select().from(ToolControlCommands),
					),
				),
			);
			const snapshot = harness.connector_snapshot();

			expect(result.approval).toMatchObject({
				decision_id: "decision_reconnect_1",
			});
			expect(snapshot.connections).toBe(2);
			expect(snapshot.dropped_tool_approval_decide_results).toBe(1);
			expect(snapshot.tool_approval_decide_attempts).toHaveLength(2);
			expect(snapshot.tool_approval_decide_attempts[1]).toEqual(
				snapshot.tool_approval_decide_attempts[0],
			);
			expect(snapshot.tool_approval_decide_attempts[0]).toMatchObject({
				payload: { decision_id: "decision_reconnect_1" },
			});
			expect(
				commands.filter(
					({ command_id, kind }) =>
						command_id === "decision_reconnect_1" && kind === "decision",
				),
			).toHaveLength(1);
			expect(state.calls).toEqual({ "tool.approval": 1 });
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	});

	it("keeps private tool data on exact completed replay while approval flow remains source-safe", async () => {
		const state = { calls: {} };
		const { harness, runtime } = await start_stack(state);

		try {
			const input = {
				arguments: { private_input: "secret-tool-input" },
				context: { agent_id: "agent_1", run_id: "run_1", thread_id: "thread_1" },
				tool: { revision: 1, tool_id: "tool.automatic" },
			};
			const eligible = await Effect.runPromise(
				harness.client.ListEligibleTools({ context: input.context }),
			);
			const pending = await Effect.runPromise(harness.client.InvokeTool(input));
			const coordinator = await runtime.runPromise(ToolControlCoordinator);

			expect(eligible.tools).toHaveLength(2);
			expect(pending).toMatchObject({ outcome: "pending" });
			expect(JSON.stringify(pending)).not.toContain("secret-tool-input");

			await runtime.runPromise(coordinator.AwaitIdle);

			const completed = await Effect.runPromise(
				harness.client.InvokeTool({ ...input, request_id: pending.invocation.request_id }),
			);
			const projection = await Effect.runPromise(
				harness.client.GetToolInvocation({
					invocation_id: pending.invocation.invocation_id,
					thread_id: "thread_1",
				}),
			);

			expect(completed).toMatchObject({
				outcome: "completed",
				result: { private_result: "secret-tool-result" },
			});
			expect(JSON.stringify(projection)).not.toContain("secret-tool-result");

			const approval_required = await Effect.runPromise(
				harness.client.InvokeTool({
					...input,
					tool: { revision: 1, tool_id: "tool.approval" },
				}),
			);

			if (approval_required.outcome !== "approval_required") {
				throw new Error("Expected a required tool to request approval.");
			}

			const approval = await Effect.runPromise(
				harness.client.GetToolApproval({
					approval_id: approval_required.invocation.approval_id,
					thread_id: "thread_1",
				}),
			);

			expect(approval_required).toMatchObject({ outcome: "approval_required" });
			expect(approval.approval).toMatchObject({ state: "requested" });

			const decided = await Effect.runPromise(
				harness.client.DecideToolApproval({
					approval_id: approval_required.invocation.approval_id,
					decision: "approved",
					decision_id: "decision_1",
					thread_id: "thread_1",
				}),
			);

			await runtime.runPromise(coordinator.AwaitIdle);

			const approved_projection = await Effect.runPromise(
				harness.client.GetToolInvocation({
					invocation_id: approval_required.invocation.invocation_id,
					thread_id: "thread_1",
				}),
			);

			expect(decided.approval).toMatchObject({ state: "approved" });
			expect(approved_projection.invocation).toMatchObject({ state: "completed" });
			expect(JSON.stringify(approved_projection)).not.toContain("secret-tool-result");
			expect(state.calls).toEqual({ "tool.approval": 1, "tool.automatic": 1 });
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	});
});
