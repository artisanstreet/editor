import { Effect, Layer, Stream } from "effect";
import { describe, expect, it } from "vitest";

import { WorkspaceFilesystemRegistry } from "../../../modules/backend/src/filesystem/workspace-filesystem-registry";
import { GitService } from "../../../modules/backend/src/git/git-service";
import { JournalStore } from "../../../modules/backend/src/persistence/journal-store";
import { RuntimeMetadata } from "../../../modules/backend/src/runtime/runtime-metadata";
import { TerminalSessionService } from "../../../modules/backend/src/terminal/terminal-sessions";
import { ExecuteTool, ExecuteToolLive } from "../../../modules/backend/src/tools/tool-handlers";
import { WorkspaceEvidenceRecorder } from "../../../modules/backend/src/workspace/workspace-evidence-recorder";
import { WorkspaceFileDiscovery } from "../../../modules/backend/src/workspace/workspace-file-discovery";
import { WorkspaceFileService } from "../../../modules/backend/src/workspace/workspace-file-service";

const timestamp = "2026-07-18T12:00:00.000Z";

const make_runtime = (
	calls: Array<{ readonly name: string; readonly value: unknown }>,
	deny_authorization = false,
) => {
	const layer = Layer.mergeAll(
		Layer.succeed(RuntimeMetadata, {
			instance_id: "tools_test",
			MakeId: () => Effect.succeed("message_generated"),
			Now: Effect.succeed(timestamp),
		}),
		Layer.succeed(WorkspaceFileService, {
			Read: (value: unknown) =>
				Effect.sync(() => {
					calls.push({ name: "read", value });
				}),
			Replace: (value: unknown) =>
				Effect.sync(() => {
					calls.push({ name: "replace", value });
				}),
		} as never),
		Layer.succeed(WorkspaceFileDiscovery, {
			Discover: (value: unknown) =>
				Effect.sync(() => {
					calls.push({ name: "discover", value });
					return { entries: [], truncated: false, workspace_id: "workspace_1" };
				}),
			LanguageCapabilities: () =>
				Effect.succeed({ capabilities: [], workspace_id: "workspace_1" }),
		} as never),
		Layer.succeed(GitService, {
			Diff: (value: unknown) =>
				Effect.sync(() => {
					calls.push({ name: "diff", value });
				}),
			Query: (value: unknown) =>
				Effect.sync(() => {
					calls.push({ name: "query", value });
				}),
			Request: (value: unknown) =>
				Effect.sync(() => {
					calls.push({ name: "request", value });
				}),
			Resolve: (value: unknown) =>
				Effect.sync(() => {
					calls.push({ name: "resolve", value });
				}),
		} as never),
		Layer.succeed(WorkspaceFilesystemRegistry, {
			Authorize: (value: unknown) =>
				Effect.sync(() => calls.push({ name: "authorize", value })).pipe(
					Effect.flatMap(() =>
						deny_authorization
							? Effect.fail(new Error("unauthorized root"))
							: Effect.succeed({}),
					),
				),
		} as never),
		Layer.succeed(TerminalSessionService, {
			Handle: (value: unknown) =>
				Effect.sync(() => {
					calls.push({ name: "terminal", value });
					return { terminal: { working_directory: "src" } };
				}),
			Output: () => Effect.succeed(Stream.empty),
		} as never),
		Layer.succeed(WorkspaceEvidenceRecorder, {
			RecordProcessOwnership: (value: unknown) =>
				Effect.sync(() => {
					calls.push({ name: "evidence", value });
				}),
		} as never),
		Layer.succeed(JournalStore, {
			AppendEvent: (value: unknown) =>
				Effect.sync(() => {
					calls.push({ name: "event", value });
				}),
		} as never),
	);
	return ExecuteToolLive.pipe(Layer.provide(layer));
};

describe("ExecuteTool", () => {
	it("strips the control-plane tool discriminator from strict file discovery inputs", async () => {
		const calls: Array<{ readonly name: string; readonly value: unknown }> = [];
		const service = await Effect.runPromise(
			Effect.service(ExecuteTool).pipe(Effect.provide(make_runtime(calls))),
		);
		const outcome = await Effect.runPromise(
			service.Execute({
				input: {
					tool_id: "workspace.file.list",
					workspace_id: "workspace_1",
					prefix: "src",
					limit: 10,
				},
				invocation_id: "invocation_1",
				thread_id: "thread_1",
			}),
		);
		expect(outcome).toMatchObject({ status: "succeeded" });
		expect(calls).toContainEqual({
			name: "discover",
			value: { workspace_id: "workspace_1", prefix: "src", limit: 10 },
		});
	});

	it("strips the discriminator from strict controlled file reads", async () => {
		const calls: Array<{ readonly name: string; readonly value: unknown }> = [];
		const service = await Effect.runPromise(
			Effect.service(ExecuteTool).pipe(Effect.provide(make_runtime(calls))),
		);
		await Effect.runPromise(
			service.Execute({
				input: {
					tool_id: "workspace.file.read",
					workspace_id: "workspace_1",
					path: "src/a.ts",
				},
				invocation_id: "invocation_read",
				thread_id: "thread_1",
			}),
		);
		expect(calls).toContainEqual({
			name: "read",
			value: { workspace_id: "workspace_1", path: "src/a.ts" },
		});
	});

	it("authorizes terminal roots and records process evidence for every process control action", async () => {
		const calls: Array<{ readonly name: string; readonly value: unknown }> = [];
		const service = await Effect.runPromise(
			Effect.service(ExecuteTool).pipe(Effect.provide(make_runtime(calls))),
		);
		for (const input of [
			{
				tool_id: "terminal.open" as const,
				workspace_id: "workspace_1",
				terminal_id: "terminal_1",
				executable: "node",
				args: [],
				cols: 80,
				rows: 24,
				working_directory: "src",
			},
			{ tool_id: "terminal.write" as const, terminal_id: "terminal_1", data: "x" },
			{ tool_id: "terminal.restart" as const, terminal_id: "terminal_1" },
			{ tool_id: "terminal.stop" as const, terminal_id: "terminal_1" },
		])
			await Effect.runPromise(
				service.Execute({
					input,
					invocation_id: `invocation_${input.tool_id}`,
					thread_id: "thread_1",
				}),
			);
		expect(calls.filter(({ name }) => name === "authorize")).toHaveLength(1);
		expect(calls.filter(({ name }) => name === "terminal")).toHaveLength(4);
		expect(calls.filter(({ name }) => name === "evidence")).toHaveLength(4);
	});

	it("does not dispatch a terminal when its workspace root authorization fails", async () => {
		const calls: Array<{ readonly name: string; readonly value: unknown }> = [];
		const service = await Effect.runPromise(
			Effect.service(ExecuteTool).pipe(Effect.provide(make_runtime(calls, true))),
		);
		const outcome = await Effect.runPromise(
			service.Execute({
				input: {
					tool_id: "terminal.open",
					workspace_id: "workspace_1",
					terminal_id: "terminal_1",
					executable: "node",
					args: [],
					cols: 80,
					rows: 24,
					working_directory: "src",
				},
				invocation_id: "invocation_denied_terminal",
				thread_id: "thread_1",
			}),
		);
		expect(outcome).toMatchObject({ status: "failed" });
		expect(calls.map(({ name }) => name)).toEqual(["authorize"]);
	});

	it("routes narrow Git mutations through exactly one request and resolution", async () => {
		const calls: Array<{ readonly name: string; readonly value: unknown }> = [];
		const service = await Effect.runPromise(
			Effect.service(ExecuteTool).pipe(Effect.provide(make_runtime(calls))),
		);
		await Effect.runPromise(
			service.Execute({
				input: {
					tool_id: "git.index.stage",
					workspace_id: "workspace_1",
					approval_id: "approval_1",
					mutation_id: "mutation_1",
					expected_snapshot_id: "a".repeat(64),
					expected_workspace_version: 1,
					paths: ["src/a.ts"],
				},
				invocation_id: "invocation_stage",
				thread_id: "thread_1",
			}),
		);
		expect(calls.filter(({ name }) => name === "request")).toHaveLength(1);
		expect(calls.filter(({ name }) => name === "resolve")).toHaveLength(1);
	});

	it("uses stable invocation-scoped journal idempotency keys for assumptions and native actions", async () => {
		const calls: Array<{ readonly name: string; readonly value: unknown }> = [];
		const service = await Effect.runPromise(
			Effect.service(ExecuteTool).pipe(Effect.provide(make_runtime(calls))),
		);
		for (const input of [
			{
				tool_id: "assumption.record" as const,
				assumption_id: "assumption_1",
				statement: "safe",
			},
			{ tool_id: "engine.native_action.record" as const, action: "search" },
		])
			await Effect.runPromise(
				service.Execute({
					input,
					invocation_id: "invocation_journal",
					thread_id: "thread_1",
				}),
			);
		expect(
			calls
				.filter(({ name }) => name === "event")
				.map(
					({ value }) => (value as { readonly idempotency_key: string }).idempotency_key,
				),
		).toEqual([
			"invocation_journal:assumption.record",
			"invocation_journal:engine.native_action.record",
		]);
	});
});
