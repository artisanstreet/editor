import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const Read = (path: string) => readFileSync(resolve(path), "utf8");

const Between = (source: string, start: string, end: string) =>
	source.slice(source.indexOf(start), source.indexOf(end, source.indexOf(start)));

describe("thread action receipt performance", () => {
	it("returns approval, cancellation, and question commands at their durable receipt", () => {
		const route = Read("modules/frontend/src/routes/components/thread-route.svelte");
		const approval = Between(route, "const RespondApproval", "const ResolveUsageInterruption");
		const cancellation = Between(route, "const CancelRun", "const RetryRun");
		const question = Between(route, "const RunCommand", "const settled_lifecycles");
		const respond_question = Between(
			route,
			"const RespondQuestion",
			"let pending_first_submission",
		);

		for (const action of [approval, cancellation, question]) {
			expect(action).toContain("SubmitDurableCommand(");
			expect(action).toContain("() => ScheduleInteractionRefresh");
			expect(action.match(/client\.Command\(/g)).toHaveLength(1);
			expect(action).not.toContain("yield* RefreshInteractionContext");
			expect(action).not.toContain("yield* Resync");
		}
		expect(respond_question).toContain("yield* RunCommand({");
		expect(respond_question).not.toContain("client.Command(");
	});

	it("schedules success and rejection reconciliation in the route scope", () => {
		const route = Read("modules/frontend/src/routes/components/thread-route.svelte");
		const usage = Between(route, "const ResolveUsageInterruption", "const CancelRun");
		const scheduling = Between(
			route,
			"const ScheduleConversationAndInteractionReconciliation =",
			"const RefreshSession",
		);

		expect(usage).toContain("SubmitDurableCommand(");
		expect(usage.match(/client\.Command\(/g)).toHaveLength(1);
		expect(usage).toContain("yield* ScheduleConversationAndInteractionReconciliation;");
		expect(usage).not.toContain("yield* ReconcileConversationAndInteraction;");
		expect(usage).toContain("return yield* Effect.fail(error);");
		expect(scheduling).toContain("Effect.forkIn(thread_scope)");
		expect(scheduling).toContain("Effect.ignore");
	});

	it("keeps reconciliation reads single-flight behind the existing route-owned deferreds", () => {
		const route = Read("modules/frontend/src/routes/components/thread-route.svelte");
		const refresh = Between(
			route,
			"const RefreshInteractionContextOnce",
			"const ScheduleInteractionRefresh",
		);

		/** One read at a time, and exactly one call site issuing it. */
		expect(refresh).toContain("Ref.modify(interaction_refresh_in_flight");
		expect(refresh.match(/client\.GetThreadWork/g)).toHaveLength(1);
		/**
		 * A trigger arriving mid-read must not adopt that read's answer: it was
		 * observed before the trigger happened, and every trigger here reports a
		 * change that just landed. Adopting it is what left the composer offering
		 * to stop a run the projection had already settled.
		 */
		expect(refresh).toContain("interaction_refresh_queued");
		expect(refresh).toContain("Ref.getAndSet(");
		expect(refresh).not.toContain(
			"if (claimed !== deferred) return yield* Deferred.await(claimed)",
		);
		expect(route).toContain("const ReconcileConversationAndInteraction = Effect.all(");
		expect(route).toContain('{ concurrency: "unbounded", discard: true }');
	});
});
