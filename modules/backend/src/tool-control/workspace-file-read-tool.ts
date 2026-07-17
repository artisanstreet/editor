import { Effect, Schema } from "effect";

import { ToolInputSchema, WorkspaceFileReadQueryResult, WorkspacePath } from "@artisan/protocol";

import { WorkspaceBoundedRegularFileStoreRegistry } from "../filesystem/workspace-bounded-regular-file-store-registry";
import { WorkspaceFileService } from "../workspace/workspace-file-service";
import { make_effect_tool_adapter } from "./internal/effect-tool-adapter";
import { ToolIneligible, type ToolRegistration } from "./tool-registry";

const WorkspaceFileReadArguments = Schema.Struct({
	path: WorkspacePath,
});

/** Registers the canonical source-safe workspace file read tool. */
export const WorkspaceFileReadTool = Effect.gen(function* () {
	const registry = yield* WorkspaceBoundedRegularFileStoreRegistry;
	const files = yield* WorkspaceFileService;
	const adapter = make_effect_tool_adapter({
		handler: (invocation, arguments_) => {
			const { context } = invocation;

			if (context.workspace_id === undefined) {
				return Effect.fail(new ToolIneligible({ reason_code: "workspace.required" }));
			}

			return files.Read({
				path: arguments_.path,
				workspace_id: context.workspace_id,
			});
		},
		parameters: WorkspaceFileReadArguments,
		success: WorkspaceFileReadQueryResult,
	});
	const descriptor = {
		approval_policy: "automatic",
		effect: "read",
		input_schema: Schema.decodeUnknownSync(ToolInputSchema)(adapter.input_schema),
		label: "Workspace file",
		revision: 1,
		source: "artisan",
		summary: "Read a file from the current workspace.",
		tool_id: "workspace.file.read",
	} satisfies ToolRegistration["descriptor"];
	const IsEligible = (context: Parameters<ToolRegistration["IsEligible"]>[0]) => {
		if (context.workspace_id === undefined) {
			return Effect.fail(new ToolIneligible({ reason_code: "workspace.required" }));
		}

		return registry.Get(context.workspace_id).pipe(
			Effect.asVoid,
			Effect.mapError(() => new ToolIneligible({ reason_code: "workspace.unavailable" })),
		);
	};

	return { adapter, descriptor, IsEligible, recovery_policy: "retry" } satisfies ToolRegistration;
});
