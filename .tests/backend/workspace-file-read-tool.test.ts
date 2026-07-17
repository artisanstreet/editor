import { Effect, Layer } from "effect";
import { describe, expect, it } from "vitest";

import type {
	ToolDescriptorReference,
	WorkspaceFileReadQuery,
	WorkspaceFileReadQueryResult,
} from "@artisan/protocol";

import { WorkspaceBoundedRegularFileStoreRegistry } from "../../modules/backend/src/filesystem/workspace-bounded-regular-file-store-registry";
import {
	ToolRegistry,
	make_tool_registry_layer,
} from "../../modules/backend/src/tool-control/tool-registry";
import { WorkspaceFileReadTool } from "../../modules/backend/src/tool-control/workspace-file-read-tool";
import {
	WorkspaceFileService,
	WorkspaceFileServiceError,
} from "../../modules/backend/src/workspace/workspace-file-service";

const context = {
	agent_id: "agent",
	run_id: "run",
	thread_id: "thread",
	workspace_id: "workspace-a",
};

function tool_invocation(tool: ToolDescriptorReference) {
	return { context, invocation_id: "invocation", tool };
}

const result: WorkspaceFileReadQueryResult = {
	content: "export const answer = 42;",
	identity: {
		algorithm: "sha256",
		byte_count: 25,
		content_hash: "a".repeat(64),
	},
	path: "src/answer.ts",
	workspace_id: "workspace-a",
};

function registry_layer(
	workspace_ids: ReadonlyArray<string>,
	read_result: WorkspaceFileReadQueryResult | null = result,
	read_queries: Array<WorkspaceFileReadQuery> = [],
) {
	const workspaces = new Set(workspace_ids);
	const workspace_registry = Layer.succeed(WorkspaceBoundedRegularFileStoreRegistry, {
		Authorize: () => Effect.die("unused"),
		Get: (workspace_id: string) =>
			workspaces.has(workspace_id)
				? Effect.succeed({
						reader: { ReadRegularFile: () => Effect.succeed(new Uint8Array()) },
						workspace_id,
					})
				: Effect.fail({
						_tag: "WorkspaceBoundedRegularFileStoreNotFoundError",
						workspace_id,
					}),
		ListWorkspaceIds: Effect.succeed([...workspaces]),
	} as typeof WorkspaceBoundedRegularFileStoreRegistry.Service);
	const workspace_files = Layer.succeed(WorkspaceFileService, {
		ExecuteApproved: () => Effect.die("unused"),
		Read: (query) =>
			Effect.sync(() => read_queries.push(query)).pipe(
				Effect.andThen(
					read_result === null
						? Effect.fail(
								new WorkspaceFileServiceError({
									operation: "read",
									reason: "failed",
								}),
							)
						: Effect.succeed(read_result),
				),
			),
		Review: () => Effect.die("unused"),
		Rollback: () => Effect.die("unused"),
		Replace: () => Effect.die("unused"),
		SettleDeniedApproval: () => Effect.die("unused"),
	});

	return WorkspaceFileReadTool.pipe(
		Effect.provide(Layer.merge(workspace_registry, workspace_files)),
		Effect.map((tool) => make_tool_registry_layer([tool])),
	);
}

function registry(
	workspace_ids: ReadonlyArray<string>,
	read_result: WorkspaceFileReadQueryResult | null = result,
	read_queries: Array<WorkspaceFileReadQuery> = [],
) {
	return registry_layer(workspace_ids, read_result, read_queries).pipe(
		Effect.flatMap((layer) => Effect.service(ToolRegistry).pipe(Effect.provide(layer))),
	);
}

describe("WorkspaceFileReadTool", () => {
	it("publishes the bounded descriptor and path-only schema", async () => {
		const service = await Effect.runPromise(registry(["workspace-a"]));
		const descriptor = await Effect.runPromise(
			service.Resolve({ revision: 1, tool_id: "workspace.file.read" }),
		);

		expect(descriptor).toMatchObject({
			approval_policy: "automatic",
			effect: "read",
			label: "Workspace file",
			revision: 1,
			source: "artisan",
			summary: "Read a file from the current workspace.",
			tool_id: "workspace.file.read",
		});
		expect(descriptor.input_schema).toMatchObject({
			properties: { path: { type: "string" } },
			required: ["path"],
			type: "object",
		});
	});

	it("reads through the registered context workspace and preserves the query result shape", async () => {
		const read_queries: Array<WorkspaceFileReadQuery> = [];
		const service = await Effect.runPromise(
			registry(["workspace-a", "workspace-b"], result, read_queries),
		);
		const value = await Effect.runPromise(
			service.Invoke(tool_invocation({ revision: 1, tool_id: "workspace.file.read" }), {
				path: "src/answer.ts",
			}),
		);

		expect(value).toEqual(result);
		expect(read_queries).toEqual([{ path: "src/answer.ts", workspace_id: "workspace-a" }]);
	});

	it("rejects workspace attribution overrides and malformed paths", async () => {
		const service = await Effect.runPromise(registry(["workspace-a"]));
		const failures = await Effect.runPromise(
			Effect.forEach(
				[
					{ path: "src/answer.ts", workspace_id: "workspace-b" },
					{ path: "../private.txt" },
					{ path: 42 },
				],
				(arguments_) =>
					service
						.Invoke(
							tool_invocation({ revision: 1, tool_id: "workspace.file.read" }),
							arguments_,
						)
						.pipe(Effect.flip),
			),
		);

		expect(failures.map(({ reason_code }) => reason_code)).toEqual([
			"invalid_arguments",
			"invalid_arguments",
			"invalid_arguments",
		]);
	});

	it("reports stable eligibility reasons for missing and unregistered workspaces", async () => {
		const service = await Effect.runPromise(registry(["workspace-a"]));
		const missing = await Effect.runPromise(
			service.List({ agent_id: "agent", run_id: "run", thread_id: "thread" }),
		);
		const eligible = await Effect.runPromise(service.List(context));
		const unavailable = await Effect.runPromise(
			service.List({ ...context, workspace_id: "workspace-b" }),
		);

		expect(missing.tools[0]).toMatchObject({
			reason_code: "workspace.required",
			state: "unavailable",
		});
		expect(eligible.tools[0]).toMatchObject({
			descriptor: { tool_id: "workspace.file.read" },
			state: "eligible",
		});
		expect(unavailable.tools[0]).toMatchObject({
			reason_code: "workspace.unavailable",
			state: "unavailable",
		});
	});

	it("maps workspace service failures to source-safe public registry errors", async () => {
		const service = await Effect.runPromise(registry(["workspace-a"], null));

		const failed = await Effect.runPromise(
			service
				.Invoke(tool_invocation({ revision: 1, tool_id: "workspace.file.read" }), {
					path: "src/answer.ts",
				})
				.pipe(Effect.flip),
		);

		expect(failed.reason_code).toBe("execution_failed");
	});
});
