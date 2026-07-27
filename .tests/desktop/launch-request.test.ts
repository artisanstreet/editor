import { Option } from "effect";
import { describe, expect, it } from "vitest";

import {
	DecodeForgeStartLaunchRequest,
	FindForgeStartLaunchRequest,
} from "../../modules/desktop/src/launch-request";

describe("desktop Forge launch request", () => {
	it("accepts only the fixed argument-free deep link", () => {
		expect(Option.isSome(DecodeForgeStartLaunchRequest("artisan://forge/start"))).toBe(true);

		for (const candidate of [
			"artisan://forge/start?command=calc",
			"artisan://forge/start#token",
			"artisan://forge/stop",
			"artisan://other/start",
			"https://forge/start",
			"not a URL",
		]) {
			expect(Option.isNone(DecodeForgeStartLaunchRequest(candidate))).toBe(true);
		}
	});

	it("finds the launch request without interpreting adjacent process arguments", () => {
		expect(
			Option.isSome(
				FindForgeStartLaunchRequest([
					"C:\\Program Files\\Artisan Editor\\Artisan Editor.exe",
					"--untrusted=value",
					"artisan://forge/start",
				]),
			),
		).toBe(true);
	});
});
