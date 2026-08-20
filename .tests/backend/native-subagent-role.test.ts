import { describe, expect, it } from "@effect/vitest";

import { friendly_native_role } from "../../modules/backend/src/orchestration/internal/graph-context";

describe("native subagent role presentation", () => {
	it.each([
		[undefined, "Handyman"],
		["general-purpose", "Handyman"],
		["Explore", "Explorer"],
		["plan", "Planner"],
		["researcher", "Researcher"],
		["/root/security_reviewer", "Security Reviewer"],
	])("normalizes %s to %s", (native_role, expected) => {
		expect(friendly_native_role(native_role)).toBe(expected);
	});
});
