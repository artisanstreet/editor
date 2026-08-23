import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const Read = (path: string) => readFileSync(resolve(path), "utf8");

describe("context-usage SER remediation", () => {
	it("owns run usage through the authoritative subscription controller", () => {
		const controller = Read("modules/frontend/src/lib/context-usage/run-usage-controller.ts");
		const route = Read("modules/frontend/src/routes/components/thread-route.svelte");

		expect(controller).toContain("SubscribeSurfaceUsageAggregate");
		expect(controller).toContain("RunAuthoritativeSubscription");
		expect(controller).toContain("Effect.gen(function* ()");
		expect(route).toContain("const run_usage = yield* RunUsageController");
		expect(controller).toContain("readonly Acquire:");
		expect(controller).toContain("current.owner_id !== owner_id");
		expect(route).toContain("const run_usage_lease = yield* run_usage.Acquire(undefined)");
		expect(route).toContain("yield* run_usage_lease.Select(next?.run_id)");
		expect(route).toContain("client.SubscribeThreadWork(thread_id)");
		expect(route).toContain("yield* Effect.addFinalizer(run_usage_lease.Release)");
		expect(route).not.toContain("GetSurfaceUsageAggregate");
		expect(route).not.toContain("RefreshContextUsage");
	});

	it("keeps a failed first-message handoff pending and exposes gauge detail to keyboards", () => {
		/** The draft, and so its retained first message, belongs to the new-thread surface. */
		const workspace = Read("modules/frontend/src/routes/components/new-thread-route.svelte");
		const route = Read("modules/frontend/src/routes/components/thread-route.svelte");
		const controller = Read("modules/frontend/src/lib/root/draft-thread.ts");
		const gauge = Read("modules/frontend/src/routes/components/context-usage-gauge.svelte");

		const retry_start = workspace.indexOf("const RetryDraftNavigation = Effect.gen");
		const retry_program = workspace.slice(
			retry_start,
			workspace.indexOf("</script>", retry_start),
		);
		expect(retry_program).toContain("const created = yield* RetryNewThreadDraft(draft_key)");
		expect(
			retry_program.indexOf("const created = yield* RetryNewThreadDraft(draft_key)"),
		).toBeLessThan(retry_program.indexOf("yield* NavigateCreatedDraft("));
		/**
		 * Entering Created is the successful handoff's normal intermediate state,
		 * so it cannot by itself mount the recovery notice. Only an already-retained
		 * draft or a rejected navigation reveals it.
		 */
		expect(workspace).toContain(
			'let show_draft_recovery = $state(opening_state._tag === "Created");',
		);
		expect(workspace).toContain("show_draft_recovery = false;");
		expect(workspace).toContain("show_draft_recovery = true;");
		expect(workspace).toContain("{#if locked && show_draft_recovery}");
		expect(workspace).not.toContain("{#if locked}\n\t\t<div");
		expect(route).toContain("const draft_thread = yield* DraftThreadController");
		expect(route).toContain("draft_thread.AwaitPendingSubmissionClaim(thread_id)");
		expect(route).toContain("AwaitPendingSubmissionClaim");
		const scoped_claim = controller.slice(
			controller.indexOf("const AwaitPendingSubmissionClaim"),
			controller.indexOf("return DraftThreadController.of({"),
		);
		const route_claim = route.slice(
			route.indexOf("const ClaimPendingFirstSubmission"),
			route.indexOf("const DeliverPendingFirstSubmission"),
		);
		/**
		 * The controller owns the claim lifecycle: it restores interruption only
		 * for the wait, then registers Release before returning the claim. The
		 * route must consume that boundary rather than duplicating an uncovered
		 * finalizer around its local mirror.
		 */
		expect(scoped_claim).toContain("Effect.uninterruptibleMask((restore)");
		expect(scoped_claim).toContain("yield* Effect.addFinalizer(() => claim.Release);");
		expect(scoped_claim.indexOf("yield* restore(")).toBeLessThan(
			scoped_claim.indexOf("yield* Effect.addFinalizer(() => claim.Release)"),
		);
		expect(route_claim).toContain("draft_thread.AwaitPendingSubmissionClaim(thread_id)");
		expect(route_claim).not.toContain("Effect.addFinalizer");
		expect(route_claim).not.toContain("Effect.uninterruptibleMask");
		expect(route).toContain("claimed.command_id");
		expect(route).toContain("yield* SendMessage(claimed.submission, claimed.command_id)");
		expect(route.indexOf("yield* claimed.Complete")).toBeGreaterThan(
			route.indexOf("yield* SendMessage(claimed.submission, claimed.command_id)"),
		);
		expect(route).toContain("first_submission_blocked");
		expect(route).not.toContain("The first message is being delivered.");
		expect(route).toContain("{#if pending_first_submission_error !== undefined}");
		expect(route).toContain("Retry first message");
		/**
		 * The gauge stands beside the picker as its own control, so the reading
		 * hangs off its own focusable trigger. The description must exist whether
		 * or not the tooltip is open — tooltip content is not rendered until it
		 * is shown, which would leave a focused trigger announcing nothing.
		 */
		expect(gauge).toContain("<button");
		expect(gauge).toContain('aria-describedby="context-usage-details"');
		expect(gauge).toContain('id="context-usage-details" class="sr-only"');
		expect(gauge).toContain("{description}");
	});
});
