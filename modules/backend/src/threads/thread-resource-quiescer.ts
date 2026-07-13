import { Context, Data, Effect, Layer } from "effect";

import { AgentGraphOrchestrator } from "../orchestration/agent-graph-orchestrator";
import { AgentOrchestrator } from "../orchestration/agent-orchestrator";
import { TerminalSessionService } from "../terminal/terminal-sessions";
import { WorkspaceReplaceApprovalCoordinator } from "../workspace/workspace-replace-approval-coordinator";

/** Wraps a failure to stop live resources before durable thread erasure. */
export class ThreadResourceQuiescenceFailure extends Data.TaggedError(
	"ThreadResourceQuiescenceFailure",
)<{
	readonly cause: unknown;
}> {}

/** Stops every live backend resource capable of writing thread-owned state. */
export class ThreadResourceQuiescer extends Context.Service<
	ThreadResourceQuiescer,
	{
		readonly Quiesce: (
			thread_id: string,
		) => Effect.Effect<void, ThreadResourceQuiescenceFailure>;
	}
>()("Artisan/ThreadResourceQuiescer") {}

export const ThreadResourceQuiescerLive = Layer.effect(
	ThreadResourceQuiescer,
	Effect.gen(function* () {
		const graph = yield* AgentGraphOrchestrator;
		const orchestration = yield* AgentOrchestrator;
		const terminals = yield* TerminalSessionService;
		const workspace_approvals = yield* WorkspaceReplaceApprovalCoordinator;
		const Quiesce = (thread_id: string) =>
			Effect.all(
				[
					graph.QuiesceThread(thread_id),
					orchestration.QuiesceThread(thread_id),
					terminals.QuiesceThread(thread_id),
					workspace_approvals.QuiesceThread(thread_id),
				],
				{ concurrency: "unbounded", discard: true },
			).pipe(Effect.mapError((cause) => new ThreadResourceQuiescenceFailure({ cause })));

		return { Quiesce };
	}),
);
