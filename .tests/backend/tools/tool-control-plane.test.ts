import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { NodeCrypto } from "@effect/platform-node-shared";
import { Effect, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { make_database_layer, Database } from "../../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../../modules/backend/src/persistence/journal-notifier";
import { JournalStoreLive } from "../../../modules/backend/src/persistence/journal-store";
import { Threads } from "../../../modules/backend/src/persistence/schema";
import { RuntimeMetadata } from "../../../modules/backend/src/runtime/runtime-metadata";
import { ArtisanToolApprovalPolicyLive } from "../../../modules/backend/src/tools/approval-policy";
import {
	ArtisanBuiltInToolRegistrations,
	make_artisan_tool_capability_state_layer,
	make_artisan_tool_registry_layer,
} from "../../../modules/backend/src/tools/artisan-tool-registry";
import { ToolInvocationRepositoryLive } from "../../../modules/backend/src/tools/tool-invocation-repository";
import { ExecuteTool } from "../../../modules/backend/src/tools/tool-handlers";
import {
	ToolControlPlane,
	ToolControlPlaneLive,
} from "../../../modules/backend/src/tools/tool-control-plane";
import { WorkspaceFileDiscovery } from "../../../modules/backend/src/workspace/workspace-file-discovery";

const migrations_path = fileURLToPath(new URL("../../../modules/backend/drizzle", import.meta.url));
const directories: string[] = [];
const policy = {
	approval: "never" as const,
	allow_engine_observation: true,
	allow_git_index_write: true,
	allow_preview_control: true,
	allow_process_control: true,
	allow_workspace_read: true,
	allow_workspace_write: true,
};

let metadata_sequence = 0;

const metadata = Layer.succeed(RuntimeMetadata, {
	instance_id: "tools_test",
	MakeId: (
		prefix:
			| "agent"
			| "backend"
			| "connection"
			| "event"
			| "heartbeat"
			| "message"
			| "run"
			| "stream_ticket",
	) => Effect.sync(() => `${prefix}_${++metadata_sequence}`),
	Now: Effect.succeed("2026-07-18T12:00:00.000Z"),
});

const discovery = Layer.succeed(WorkspaceFileDiscovery, {
	Discover: (query: { readonly workspace_id: string }) =>
		Effect.succeed({ entries: [], truncated: false, workspace_id: query.workspace_id }),
	LanguageCapabilities: (query: { readonly workspace_id: string }) =>
		Effect.succeed({ capabilities: [], workspace_id: query.workspace_id }),
});

const runtime = (database_path: string, calls: string[], runtime_metadata = metadata) => {
	const executor = Layer.succeed(ExecuteTool, {
		Execute: (input: { readonly input: { readonly tool_id: string } }) =>
			Effect.sync(() => {
				calls.push(input.input.tool_id);
				return { code: "executed", status: "succeeded" as const };
			}),
	});
	const registry = make_artisan_tool_registry_layer().pipe(
		Layer.provide(ArtisanToolApprovalPolicyLive),
		Layer.provide(
			make_artisan_tool_capability_state_layer(
				ArtisanBuiltInToolRegistrations.map(({ tool_id }) => ({
					state: "available" as const,
					tool_id,
				})),
			),
		),
	);
	const base = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		runtime_metadata,
		JournalNotifierLive,
		discovery,
		executor,
		ArtisanToolApprovalPolicyLive,
		registry,
		NodeCrypto.layer,
	);
	const repository = ToolInvocationRepositoryLive.pipe(Layer.provide(base));
	return ManagedRuntime.make(
		ToolControlPlaneLive.pipe(
			Layer.provideMerge(base),
			Layer.provideMerge(repository),
			Layer.provideMerge(JournalStoreLive.pipe(Layer.provide(base))),
		),
	);
};

const seed_thread = Effect.gen(function* () {
	const database = yield* Database;
	yield* database.client.insert(Threads).values({
		created_at: "2026-07-18T12:00:00.000Z",
		thread_id: "thread_1",
		title: "thread",
		updated_at: "2026-07-18T12:00:00.000Z",
	});
});

afterEach(async () =>
	Promise.all(directories.splice(0).map((path) => rm(path, { force: true, recursive: true }))),
);

describe("ToolControlPlane", () => {
	it("never dispatches denied or unavailable invocations", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-tools-"));
		directories.push(directory);
		const calls: string[] = [];
		const service_runtime = runtime(join(directory, "artisan.db"), calls);
		try {
			const result = await service_runtime.runPromise(
				Effect.gen(function* () {
					yield* seed_thread;
					const tools = yield* ToolControlPlane;
					const denied = yield* tools.Execute({
						thread_id: "thread_1",
						request: {
							invocation_id: "invocation_denied",
							policy: { ...policy, allow_workspace_write: false },
							input: {
								tool_id: "workspace.file.write",
								workspace_id: "workspace_1",
								change_id: "change_1",
								path: "a.ts",
								content: "x",
								expected_before: {
									algorithm: "sha256",
									byte_count: 0,
									content_hash: "a".repeat(64),
								},
							},
						},
					});
					return { denied, calls };
				}),
			);
			expect(result.denied).toMatchObject({ lifecycle: "denied" });
			expect(result.calls).toEqual([]);
		} finally {
			await service_runtime.dispose();
		}
	});

	it("executes an allowed request exactly once across an exact retry", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-tools-"));
		directories.push(directory);
		const calls: string[] = [];
		const service_runtime = runtime(join(directory, "artisan.db"), calls);
		try {
			const result = await service_runtime.runPromise(
				Effect.gen(function* () {
					yield* seed_thread;
					const tools = yield* ToolControlPlane;
					const request = {
						invocation_id: "invocation_assumption",
						policy,
						input: {
							tool_id: "assumption.record" as const,
							assumption_id: "assumption_1",
							statement: "safe",
						},
					};
					return [
						yield* tools.Execute({ thread_id: "thread_1", request }),
						yield* tools.Execute({ thread_id: "thread_1", request }),
					];
				}),
			);
			expect(result).toEqual([
				expect.objectContaining({ lifecycle: "succeeded" }),
				expect.objectContaining({ lifecycle: "succeeded" }),
			]);
			expect(calls).toEqual(["assumption.record"]);
		} finally {
			await service_runtime.dispose();
		}
	});

	it("persists approval-required input privately and rejects a wrong-thread resolution before execution", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-tools-"));
		directories.push(directory);
		const calls: string[] = [];
		const service_runtime = runtime(join(directory, "artisan.db"), calls);
		try {
			const result = await service_runtime.runPromise(
				Effect.gen(function* () {
					yield* seed_thread;
					const tools = yield* ToolControlPlane;
					const pending = (yield* tools.Execute({
						thread_id: "thread_1",
						request: {
							invocation_id: "invocation_pending",
							policy: { ...policy, approval: "always" },
							input: {
								tool_id: "workspace.file.write",
								workspace_id: "workspace_1",
								change_id: "change_private",
								path: "private.ts",
								content: "private input",
								expected_before: {
									algorithm: "sha256",
									byte_count: 0,
									content_hash: "a".repeat(64),
								},
							},
						},
					})) as { readonly approval_id?: string };
					const approval_id = pending.approval_id!;
					const wrong = yield* tools
						.ResolveApproval({
							thread_id: "thread_other",
							request: {
								approval_id,
								approved: true,
								invocation_id: "invocation_pending",
								resolution_id: "resolution_wrong",
							},
						})
						.pipe(Effect.flip);
					const resolved = yield* tools.ResolveApproval({
						thread_id: "thread_1",
						request: {
							approval_id,
							approved: true,
							invocation_id: "invocation_pending",
							resolution_id: "resolution_ok",
						},
					});
					return { pending, resolved, wrong };
				}),
			);
			expect(result.pending).toMatchObject({ lifecycle: "awaiting_approval" });
			expect(result.wrong).toMatchObject({ reason: "conflict" });
			expect(result.resolved).toMatchObject({ lifecycle: "succeeded" });
			expect(calls).toEqual(["workspace.file.write"]);
		} finally {
			await service_runtime.dispose();
		}
	});

	it("retries exact caller intent across a later runtime without a second execution or approval identity", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-tools-"));
		directories.push(directory);
		const calls: string[] = [];
		const database_path = join(directory, "artisan.db");
		let first_sequence = 0;
		const first_metadata = Layer.succeed(RuntimeMetadata, {
			instance_id: "first",
			MakeId: (
				prefix:
					| "agent"
					| "backend"
					| "connection"
					| "event"
					| "heartbeat"
					| "message"
					| "run"
					| "stream_ticket",
			) => Effect.sync(() => `${prefix}_first_${++first_sequence}`),
			Now: Effect.succeed("2026-07-18T12:00:00.000Z"),
		});
		let later_sequence = 0;
		const later_metadata = Layer.succeed(RuntimeMetadata, {
			instance_id: "later",
			MakeId: (
				prefix:
					| "agent"
					| "backend"
					| "connection"
					| "event"
					| "heartbeat"
					| "message"
					| "run"
					| "stream_ticket",
			) => Effect.sync(() => `${prefix}_later_${++later_sequence}`),
			Now: Effect.succeed("2026-07-18T12:01:00.000Z"),
		});
		const request = {
			invocation_id: "retry_pending",
			policy: { ...policy, approval: "always" as const },
			input: {
				tool_id: "terminal.open" as const,
				terminal_id: "retry_terminal",
				workspace_id: "workspace",
				working_directory: "C:\\workspace",
				executable: "cmd",
				args: [],
				cols: 80,
				rows: 24,
			},
		};
		const first = runtime(database_path, calls, first_metadata);
		try {
			const initial = (await first.runPromise(
				Effect.gen(function* () {
					yield* seed_thread;
					return yield* (yield* ToolControlPlane).Execute({
						thread_id: "thread_1",
						request,
					});
				}),
			)) as { readonly approval_id?: string };
			await first.dispose();
			const later = runtime(database_path, calls, later_metadata);
			try {
				const retried = await later.runPromise(
					Effect.gen(function* () {
						return yield* (yield* ToolControlPlane).Execute({
							thread_id: "thread_1",
							request,
						});
					}),
				);
				expect(retried).toMatchObject({
					approval_id: initial.approval_id,
					lifecycle: "awaiting_approval",
				});
				expect(calls).toEqual([]);
			} finally {
				await later.dispose();
			}
		} finally {
			await first.dispose().catch(() => undefined);
		}
	});

	it("retries an allowed invocation across a later runtime without a second handler call", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-tools-"));
		directories.push(directory);
		const calls: string[] = [];
		const database_path = join(directory, "artisan.db");
		let first_sequence = 0;
		let later_sequence = 0;
		const first_metadata = Layer.succeed(RuntimeMetadata, {
			instance_id: "allowed_first",
			MakeId: (
				prefix:
					| "agent"
					| "backend"
					| "connection"
					| "event"
					| "heartbeat"
					| "message"
					| "run"
					| "stream_ticket",
			) => Effect.sync(() => `${prefix}_allowed_first_${++first_sequence}`),
			Now: Effect.succeed("2026-07-18T12:00:00.000Z"),
		});
		const later_metadata = Layer.succeed(RuntimeMetadata, {
			instance_id: "allowed_later",
			MakeId: (
				prefix:
					| "agent"
					| "backend"
					| "connection"
					| "event"
					| "heartbeat"
					| "message"
					| "run"
					| "stream_ticket",
			) => Effect.sync(() => `${prefix}_allowed_later_${++later_sequence}`),
			Now: Effect.succeed("2026-07-18T12:01:00.000Z"),
		});
		const request = {
			invocation_id: "retry_allowed",
			policy,
			input: {
				tool_id: "assumption.record" as const,
				assumption_id: "allowed_assumption",
				statement: "safe",
			},
		};
		const first = runtime(database_path, calls, first_metadata);
		try {
			const initial = await first.runPromise(
				Effect.gen(function* () {
					yield* seed_thread;
					return yield* (yield* ToolControlPlane).Execute({
						thread_id: "thread_1",
						request,
					});
				}),
			);
			expect(initial).toMatchObject({ lifecycle: "succeeded" });
			await first.dispose();
			const later = runtime(database_path, calls, later_metadata);
			try {
				const retry = await later.runPromise(
					Effect.gen(function* () {
						return yield* (yield* ToolControlPlane).Execute({
							thread_id: "thread_1",
							request,
						});
					}),
				);
				expect(retry).toMatchObject({ lifecycle: "succeeded" });
				expect(calls).toEqual(["assumption.record"]);
			} finally {
				await later.dispose();
			}
		} finally {
			await first.dispose().catch(() => undefined);
		}
	});
});
