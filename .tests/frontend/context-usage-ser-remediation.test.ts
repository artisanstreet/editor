import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const Read = (path: string) => readFileSync(resolve(path), "utf8");

describe("context-usage SER remediation", () => {
	it("owns run usage through the authoritative subscription controller", () => {
		const controller = Read("modules/frontend/src/lib/context-usage/run-usage-controller.ts");
		const route = Read("modules/frontend/src/routes/components/thread-route.sv");

		expect(controller).toContain("SubscribeSurfaceUsageAggregate");
		expect(controller).toContain("RunAuthoritativeSubscription");
		expect(controller).toContain("Effect.gen(function* ()");
		expect(route).toContain("const run_usage = yield* RunUsageController");
		expect(controller).toContain("readonly Acquire:");
		expect(controller).toContain("current.owner_id !== owner_id");
		expect(route).toContain("const run_usage_lease = yield* run_usage.Acquire(undefined)");
		expect(route).toContain("yield* run_usage_lease.Select(work?.run_id)");
		expect(route).toContain("yield* Effect.addFinalizer(run_usage_lease.Release)");
		expect(route).not.toContain("GetSurfaceUsageAggregate");
		expect(route).not.toContain("RefreshContextUsage");
	});

	it("keeps a failed first-message handoff pending and exposes gauge detail to keyboards", () => {
		const root = Read("modules/frontend/src/routes/+page.sv");
		const route = Read("modules/frontend/src/routes/components/thread-route.sv");
		const gauge = Read("modules/frontend/src/routes/components/context-usage-gauge.sv");

		const retry_start = root.indexOf("const RetryDraftNavigation = Effect.gen");
		const retry_program = root.slice(retry_start, root.indexOf("</script>", retry_start));
		expect(retry_program).toContain("const created = yield* draft_thread.Retry");
		expect(retry_program.indexOf("const created = yield* draft_thread.Retry")).toBeLessThan(
			retry_program.indexOf(
				"yield* Navigate(ThreadRoutePath(created.project.project_id, created.thread_id))",
			),
		);
		expect(route).toContain("const draft_thread = yield* DraftThreadController");
		expect(route).toContain("yield* draft_thread.AwaitPendingSubmissionClaim(thread_id)");
		expect(route).toContain("AwaitPendingSubmissionClaim");
		expect(route).toContain("claimed.command_id");
		expect(route).toContain("yield* SendMessage(claimed.submission, claimed.command_id)");
		expect(route.indexOf("yield* claimed.Complete")).toBeGreaterThan(
			route.indexOf("yield* SendMessage(claimed.submission, claimed.command_id)"),
		);
		expect(route).toContain("first_submission_blocked");
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
