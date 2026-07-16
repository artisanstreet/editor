import { Context, Data, Effect, Layer } from "effect";

import { ExternalWaitDispatcher } from "../external-wait/external-wait-dispatcher";
import { HostedGitMutationCoordinator } from "../git-provider/hosted-git-mutation-coordinator";
import { AgentGraphOrchestrator } from "../orchestration/agent-graph-orchestrator";
import { AgentOrchestrator } from "../orchestration/agent-orchestrator";
import { HostedProjectCloneCoordinator } from "../projects/hosted-project-clone-coordinator";
import { PreviewBrowserLifecycle } from "../preview/preview-browser";
import { TerminalSessionService } from "../terminal/terminal-sessions";
import { WorkspaceGitCheckoutCoordinator } from "../git/workspace-git-checkout-coordinator";
import { WorkspaceGitFetchService } from "../git/workspace-git-fetch-service";
import { WorkspaceGitMutationCoordinator } from "../git/workspace-git-mutation-coordinator";
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
		const external_waits = yield* ExternalWaitDispatcher;
		const graph = yield* AgentGraphOrchestrator;
		const hosted_git_mutations = yield* HostedGitMutationCoordinator;
		const hosted_project_clones = yield* HostedProjectCloneCoordinator;
		const orchestration = yield* AgentOrchestrator;
		const preview_browser = yield* PreviewBrowserLifecycle;
		const terminals = yield* TerminalSessionService;
		const workspace_git_checkouts = yield* WorkspaceGitCheckoutCoordinator;
		const workspace_git_fetches = yield* WorkspaceGitFetchService;
		const workspace_git_mutations = yield* WorkspaceGitMutationCoordinator;
		const workspace_approvals = yield* WorkspaceReplaceApprovalCoordinator;
		const Quiesce = (thread_id: string) =>
			Effect.all(
				[
					external_waits.QuiesceThread(thread_id),
					graph.QuiesceThread(thread_id),
					hosted_git_mutations.QuiesceThread(thread_id),
					hosted_project_clones.QuiesceThread(thread_id),
					orchestration.QuiesceThread(thread_id),
					preview_browser.QuiesceThread(thread_id),
					terminals.QuiesceThread(thread_id),
					workspace_git_checkouts.QuiesceThread(thread_id),
					workspace_git_fetches.QuiesceThread(thread_id),
					workspace_git_mutations.QuiesceThread(thread_id),
					workspace_approvals.QuiesceThread(thread_id),
				],
				{ concurrency: "unbounded", discard: true },
			).pipe(Effect.mapError((cause) => new ThreadResourceQuiescenceFailure({ cause })));

		return { Quiesce };
	}),
);
