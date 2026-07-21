import { describe, expect, it } from "vitest";

import { resolve_desktop_identity } from "@artisan/desktop";

describe("desktop identity projection", () => {
	it("uses the OS username first and provides a stable deterministic fallback seed", () => {
		expect(
			resolve_desktop_identity({
				machine_name: "artisan-station",
				username: "sander",
			}),
		).toEqual({
			avatar_seed: "sander:artisan-station",
			display_name: "sander",
			machine_name: "artisan-station",
		});
	});

	it("falls back to the machine name and strips control data before projection", () => {
		expect(
			resolve_desktop_identity({
				machine_name: " workstation\n",
				username: "\u0000",
			}),
		).toMatchObject({
			avatar_seed: "workstation:workstation",
			display_name: "workstation",
			machine_name: "workstation",
		});
	});
});
