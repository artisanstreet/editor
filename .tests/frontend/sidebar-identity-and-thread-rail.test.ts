import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { Effect } from "effect";
import { describe, expect, it } from "vitest";

const read = (path: string) => readFileSync(resolve(path), "utf8");

describe("sidebar identity and thread rail regressions", () => {
	it("forks provider usage reads with the Effect 4 scoped concurrency API", () => {
		const identity = read("modules/frontend/src/routes/components/sidebar-identity.sv");

		expect(Effect.forkScoped).toBeTypeOf("function");
		expect(identity).toContain("const MakeUsageRequests = Queue.unbounded<UsageRequest>();");
		expect(identity).toContain("const usage_requests = yield* MakeUsageRequests;");
		expect(identity).not.toMatch(/yield\*\s*Queue\.unbounded<\{/);
		expect(identity).toContain(
			"Effect.forkScoped(FetchEngineUsage(request.engine_id, request.force))",
		);
		expect(identity).not.toMatch(/\bEffect\.fork\(/);
	});

	it("keeps thread-list rows stationary while retaining proximity reveal", () => {
		const rail = read("modules/frontend/src/routes/components/thread-hover-rail.sv");
		const styles = read("modules/frontend/src/lib/styles/sidebar.css");

		expect(rail).toContain("<svelte:window onpointermove={TrackPointer} />");
		expect(rail).toContain('class="t-panel-slide-x');
		expect(rail).not.toContain("SetShifts");
		expect(rail).not.toContain("getComputedStyle");
		expect(rail).not.toContain("t-avatar");
		expect(styles).not.toContain("--avatar-");
		expect(styles).not.toContain(".t-avatar");
	});
});
