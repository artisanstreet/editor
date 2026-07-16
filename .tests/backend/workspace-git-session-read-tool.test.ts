import { promises as fs } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Effect, Layer } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	EmptyWorkspaceGitRegistryLive,
	make_node_workspace_git_registry_layer,
} from "../../modules/backend/src/git/workspace-git-registry";
import { WorkspaceGitSessionUnavailable } from "../../modules/backend/src/git/workspace-git-session-repository";
import { WorkspaceGitSessionService } from "../../modules/backend/src/git/workspace-git-session-service";
import {
	ToolRegistry,
	make_tool_registry_layer,
} from "../../modules/backend/src/tool-control/tool-registry";
import { WorkspaceGitSessionReadTool } from "../../modules/backend/src/tool-control/workspace-git-session-read-tool";

const roots: Array<string> = [];
const context = {
	agent_id: "agent",
	run_id: "run",
	thread_id: "thread",
	workspace_id: "workspace",
};

async function make_root() {
	const root = await fs.mkdtemp(join(tmpdir(), "artisan-tool-workspace-"));

	roots.push(root);

	return root;
}

function session_layer(input: {
	readonly Query: (typeof WorkspaceGitSessionService.Service)["Query"];
}) {
	const unused = Effect.die("unused");

	return Layer.succeed(WorkspaceGitSessionService, {
		Project: () => unused,
		ProjectObserved: () => unused,
		Query: input.Query,
		RecoverEvidence: unused,
		Refresh: () => unused,
	});
}

function tool_registry_layer(
	root?: string,
	sessions = session_layer({ Query: () => Effect.succeed({ journal_sequence: 1 }) }),
) {
	const git =
		root === undefined
			? EmptyWorkspaceGitRegistryLive
			: make_node_workspace_git_registry_layer([
					{ root, workspace_id: context.workspace_id },
				]);

	return WorkspaceGitSessionReadTool.pipe(
		Effect.provide(Layer.merge(git, sessions)),
		Effect.map((tool) => make_tool_registry_layer([tool])),
	);
}

function registry(root?: string, sessions?: ReturnType<typeof session_layer>) {
	return tool_registry_layer(root, sessions).pipe(
		Effect.flatMap((layer) => Effect.service(ToolRegistry).pipe(Effect.provide(layer))),
	);
}

afterEach(async () => {
	await Promise.all(roots.splice(0).map((root) => fs.rm(root, { force: true, recursive: true })));
});

describe("WorkspaceGitSessionReadTool", () => {
	it("reports source-safe workspace eligibility without querying run ownership", async () => {
		const service = await Effect.runPromise(registry());
		const missing = await Effect.runPromise(
			service.List({ agent_id: "agent", run_id: "run", thread_id: "thread" }),
		);
		const unavailable = await Effect.runPromise(service.List(context));

		expect(missing.tools[0]).toMatchObject({
			reason_code: "workspace.required",
			state: "unavailable",
		});
		expect(unavailable.tools[0]).toMatchObject({
			reason_code: "workspace.unavailable",
			state: "unavailable",
		});
	});

	it("reads the registered workspace session, with an automatic source-safe descriptor", async () => {
		const root = await make_root();
		const service = await Effect.runPromise(registry(root));
		const listed = await Effect.runPromise(service.List(context));
		const result = await Effect.runPromise(
			service.Invoke({ revision: 1, tool_id: "workspace.git.session.read" }, context, {}),
		);

		expect(listed.tools[0]).toMatchObject({ state: "eligible" });
		expect(listed.tools[0]?.descriptor).toMatchObject({
			approval_policy: "automatic",
			effect: "read",
			label: "Git session",
			source: "artisan",
		});
		expect(result).toEqual({ journal_sequence: 1 });
	});

	it("rejects argument attribution overrides before querying and keeps diagnostics out of errors", async () => {
		const root = await make_root();
		let calls = 0;
		const diagnostics = "C:\\private\\workspace\\git failure";
		const sessions = session_layer({
			Query: () => {
				calls += 1;

				return Effect.fail(new WorkspaceGitSessionUnavailable({ reason: "missing" }));
			},
		});
		const service = await Effect.runPromise(registry(root, sessions));
		const invalid_arguments = await Effect.runPromise(
			service
				.Invoke({ revision: 1, tool_id: "workspace.git.session.read" }, context, {
					workspace_id: "other-workspace",
				})
				.pipe(Effect.flip),
		);
		const failed = await Effect.runPromise(
			service
				.Invoke({ revision: 1, tool_id: "workspace.git.session.read" }, context, {})
				.pipe(Effect.flip),
		);

		expect(calls).toBe(1);
		expect(invalid_arguments.reason_code).toBe("invalid_arguments");
		expect(failed.reason_code).toBe("execution_failed");
		expect(JSON.stringify({ invalid_arguments, failed })).not.toContain(diagnostics);
	});
});
