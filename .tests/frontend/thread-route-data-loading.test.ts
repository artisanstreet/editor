import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const Read = (path: string) => readFileSync(resolve(path), "utf8");

describe("thread route data loading", () => {
	it("uses the retained workspace catalog instead of direct project or thread list reads", () => {
		const route = Read("modules/frontend/src/routes/components/thread-route.svelte");
		const start_thread = route.slice(
			route.indexOf("const StartThreadWithPrompt"),
			route.indexOf("const RunCommand"),
		);

		expect(route).toContain("yield* WorkspaceCatalogController");
		expect(route).not.toContain("workspace_catalog.Current");
		expect(route).not.toContain("workspace_catalog.RefreshProjects");
		expect(route).not.toContain("client.ListThreads");
		expect(route).not.toContain("client.ListProjects");
		expect(start_thread).toContain("draft_thread.SelectProject(project_ref)");
		expect(start_thread).toContain(
			"ThreadRoutePath(created.project.project_id, created.thread_id)",
		);
	});

	it("joins overlapping interaction refreshes through one route-owned deferred", () => {
		const route = Read("modules/frontend/src/routes/components/thread-route.svelte");
		const refresh = route.slice(
			route.indexOf("const RefreshInteractionContextOnce"),
			route.indexOf("const RefreshSession"),
		);

		expect(route).toContain("interaction_refresh_in_flight");
		expect(route).toContain("Deferred.make<void, ArtisanClientError>()");
		/**
		 * One read in flight at a time — but a caller arriving during one gets a
		 * successor read rather than that read's answer, which was taken before
		 * the change it is asking about.
		 */
		expect(route).toContain("const queued = yield* Ref.modify(interaction_refresh_queued");
		expect(route).toContain(
			"Effect.forkIn(CompleteInteractionRefresh(deferred), thread_scope)",
		);
		expect(route).not.toContain("MakeLatestRequestGate");
		expect(route).not.toContain("Semaphore.make(1)");
		expect(refresh.match(/client\.GetThreadWork/g)).toHaveLength(1);
		expect(refresh).not.toContain("client.GetThreadSession");
		expect(refresh).not.toContain("workspace_catalog.Current");
	});

	it("streams session, work, and catalog state while ignoring unrelated event traffic", () => {
		const route = Read("modules/frontend/src/routes/components/thread-route.svelte");
		const events = route.slice(
			route.lastIndexOf("RunAuthoritativeSubscription("),
			route.indexOf("const RespondQuestion"),
		);

		expect(route).toContain("client.SubscribeThreadSession(thread_id)");
		expect(route).toContain("client.SubscribeThreadWork(thread_id)");
		expect(route).toContain("workspace_catalog.Changes.pipe(Stream.runForEach(ApplyCatalog))");
		expect(events).toContain('event.payload.type === "thread.erased"');
		expect(events).not.toContain('event.payload.type === "run.lifecycle"');
		for (const unrelated of [
			"filesystem.mutation",
			"process.ownership",
			"git.workspace.observed",
			"assignment.heartbeat",
		]) {
			expect(events).not.toContain(unrelated);
		}
	});

	it("never authorizes a send from cached thread-open session or work", () => {
		const route = Read("modules/frontend/src/routes/components/thread-route.svelte");
		const send = route.slice(
			route.indexOf("const SendMessage"),
			route.indexOf("const WithdrawQueuedMessage"),
		);

		expect(route).toContain("let work_ready = $state(false);");
		expect(route).toContain("let session_ready = $state(false);");
		expect(route).toContain("work_ready &&");
		expect(route).toContain(
			"disabled={!session_ready || !work_ready || first_submission_blocked}",
		);
		expect(send).toContain("Effect.all([AwaitSessionAuthority, AwaitWorkAuthority]");
		expect(route).toContain(
			"if (!work_ready) yield* ApplyWorkValue(Option.getOrUndefined(next_work));",
		);
	});

	it("coalesces conversation resyncs and runs independent recovery reads concurrently", () => {
		const route = Read("modules/frontend/src/routes/components/thread-route.svelte");

		expect(route).toContain("conversation_resync_in_flight");
		expect(route).toContain("Effect.forkIn(CompleteResync(deferred), thread_scope)");
		expect(route).toContain("const ReconcileConversationAndInteraction = Effect.all(");
		expect(route).toContain("const ScheduleConversationAndInteractionReconciliation =");
		expect(route).toContain('{ concurrency: "unbounded", discard: true }');
	});

	it("does not let the agents card reread the route's authoritative session", () => {
		const agents = Read("modules/frontend/src/routes/components/thread-agents.svelte");
		const route = Read("modules/frontend/src/routes/components/thread-route.svelte");

		expect(route).toContain("session_projection.Publish(next)");
		expect(agents).toContain("yield* ThreadSessionProjection");
		expect(agents).toContain("session_projection.Changes");
		expect(agents).toContain("const policy = $derived(session?.policy)");
		expect(agents).not.toContain("ArtisanClient");
		expect(agents).not.toContain("GetThreadSession");
	});

	it("forks a claimed first submission after hydration instead of awaiting message delivery", () => {
		const route = Read("modules/frontend/src/routes/components/thread-route.svelte");
		const delivery = route.slice(
			route.indexOf("const DeliverPendingFirstSubmission"),
			route.indexOf("</script>"),
		);

		expect(delivery).toContain(
			"const DeliverClaimedFirstSubmission = DeliverPendingFirstSubmission.pipe(",
		);
		expect(delivery).toContain("const claim = yield* ClaimPendingFirstSubmission;");
		expect(delivery).toContain("if (claim === undefined) return;");
		expect(delivery).toContain(
			"yield* Effect.forkIn(DeliverClaimedFirstSubmission, thread_scope);",
		);
		expect(delivery).not.toContain("yield* DeliverPendingFirstSubmission.pipe(");
		expect(delivery.indexOf("yield* claimed.Complete")).toBeGreaterThan(
			delivery.indexOf("yield* SendMessage(claimed.submission, claimed.command_id)"),
		);
		expect(delivery).toContain("pending_first_submission_error = error.message;");
		expect(delivery).toContain("first_submission_blocked = true;");
		expect(delivery).toContain("const ClaimAndDeliverFirstSubmissionRetry = Effect.gen");
		expect(delivery).toContain("yield* ClaimPendingFirstSubmission;");
		expect(delivery).toContain("Effect.ensuring(FinishFirstSubmissionRetry)");
		expect(delivery).toContain(
			"yield* Effect.forkIn(ClaimAndDeliverFirstSubmissionRetry, thread_scope);",
		);
		expect(route).toContain("{#if pending_first_submission_error !== undefined}");
		expect(route).toContain("Retry first message");
	});

	it("projects an accepted policy receipt locally instead of reloading the route", () => {
		const route = Read("modules/frontend/src/routes/components/thread-route.svelte");
		const update = route.slice(
			route.indexOf("const UpdateSessionPolicy"),
			route.indexOf("const PersistSessionPolicy"),
		);
		const session_subscription = route.slice(
			route.indexOf("RunAuthoritativeSubscription(\n\t\t\tclient.SubscribeThreadSession"),
			route.indexOf(
				"RunAuthoritativeSubscription(",
				route.indexOf(
					"RunAuthoritativeSubscription(\n\t\t\tclient.SubscribeThreadSession",
				) + 1,
			),
		);

		expect(update).toContain("const receipt = yield* client.UpdateThreadSessionPolicy");
		expect(update).toContain("journal_sequence: receipt.journal_sequence");
		expect(update).not.toContain("client.GetThreadSession");
		expect(update).not.toContain("yield* RefreshInteractionContext");
		expect(route).toContain("const RefreshSession = client.GetThreadSession(thread_id)");
		expect(session_subscription).toContain("client.SubscribeThreadSession(thread_id)");
		expect(session_subscription).toContain("RefreshSession");
	});
});
