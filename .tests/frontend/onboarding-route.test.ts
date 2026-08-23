import { describe, expect, it } from "vitest";

import { ShouldRedirectToOnboarding } from "../../modules/frontend/src/lib/onboarding-route";

describe("onboarding route gate", () => {
	it("waits for authoritative defaults before redirecting", () => {
		expect(
			ShouldRedirectToOnboarding({
				completed: undefined,
				defaults_available: false,
				pathname: "/",
			}),
		).toBe(false);
	});

	it("redirects unfinished ordinary routes", () => {
		expect(
			ShouldRedirectToOnboarding({
				completed: false,
				defaults_available: true,
				pathname: "/settings",
			}),
		).toBe(true);
	});

	it("allows completed, onboarding, and debug routes", () => {
		for (const input of [
			{ completed: true, defaults_available: true, pathname: "/" },
			{ completed: false, defaults_available: true, pathname: "/onboarding" },
			{ completed: false, defaults_available: true, pathname: "/debug/onboarding" },
		] as const)
			expect(ShouldRedirectToOnboarding(input)).toBe(false);
	});
});
