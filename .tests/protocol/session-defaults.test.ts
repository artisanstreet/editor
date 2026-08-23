import { Effect, Schema } from "effect";
import { describe, expect, it } from "vitest";

import {
	AgentNameDataset,
	AgentNameDatasetIds,
	AgentNameDatasets,
	NormalizePermissionId,
} from "@artisan/protocol";

describe("agent name datasets", () => {
	it("exposes only the Norwegian and restored British catalogs", () => {
		expect(AgentNameDatasetIds).toEqual(["norwegian", "british"]);
		expect(AgentNameDatasets.map(({ id }) => id)).toEqual(["norwegian", "british"]);
	});

	it("migrates the removed playful preference to the British catalog", async () => {
		const decoded = await Effect.runPromise(
			Schema.decodeUnknownEffect(AgentNameDataset)("playful"),
		);

		expect(decoded).toBe("british");
	});
});

describe("permission compatibility", () => {
	it("collapses retired five-step choices onto Auto", () => {
		expect(NormalizePermissionId("supervised")).toBe("autonomous");
		expect(NormalizePermissionId("trusted")).toBe("autonomous");
		expect(NormalizePermissionId("restricted")).toBe("restricted");
		expect(NormalizePermissionId("unrestricted")).toBe("unrestricted");
	});
});
