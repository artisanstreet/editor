import { Effect, Schema } from "effect";
import { Tool } from "effect/unstable/ai";

import { ToolInputSchema, WorkspaceGitSessionQueryResult } from "@artisan/protocol";

import { WorkspaceGitRegistry } from "../git/workspace-git-registry";
import { WorkspaceGitSessionService } from "../git/workspace-git-session-service";
import { make_effect_tool_adapter } from "./internal/effect-tool-adapter";
import { ToolIneligible, type ToolRegistration } from "./tool-registry";

/** Registers the canonical source-safe workspace Git session read tool. */
export const WorkspaceGitSessionReadTool = Effect.gen(function* () {
	const registry = yield* WorkspaceGitRegistry;
	const sessions = yield* WorkspaceGitSessionService;
	const adapter = make_effect_tool_adapter({
		handler: (invocation) => {
			const { context } = invocation;

			if (context.workspace_id === undefined) {
				return Effect.fail(new ToolIneligible({ reason_code: "workspace.required" }));
			}

			return sessions.Query({ workspace_id: context.workspace_id });
		},
		parameters: Tool.EmptyParams,
		success: WorkspaceGitSessionQueryResult,
	});
	const descriptor = {
		approval_policy: "automatic",
		effect: "read",
		input_schema: Schema.decodeUnknownSync(ToolInputSchema)(adapter.input_schema),
		label: "Git session",
		revision: 1,
		source: "artisan",
		summary: "Read the current workspace Git session.",
		tool_id: "workspace.git.session.read",
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
