import { afterEach, describe, expect, it, vi } from "vitest";
import { Effect, Option } from "effect";

import { DiscoverForge, DiscoverForgeHealth } from "../../modules/frontend/src/lib/forge/discovery";
import { ForgeEndpointStoreLive } from "../../modules/frontend/src/lib/runtime/forge-endpoint";

afterEach(() => {
	vi.unstubAllGlobals();
});

describe("Forge discovery", () => {
	it("schema-decodes health and filters the current instance", async () => {
		vi.stubGlobal(
			"fetch",
			vi
				.fn()
				.mockResolvedValueOnce(
					new Response(JSON.stringify({ development: true }), { status: 200 }),
				)
				.mockResolvedValueOnce(
					new Response(
						JSON.stringify({
							instances: [
								{ endpoint: "http://127.0.0.1:4100", self: true },
								{ endpoint: "http://127.0.0.1:4200", self: false },
							],
						}),
						{ status: 200 },
					),
				),
		);

		const discovered = await Effect.runPromise(
			DiscoverForge.pipe(Effect.provide(ForgeEndpointStoreLive)),
		);
		expect(Option.getOrThrow(discovered.health).development).toBe(true);
		expect(discovered.others.map((instance) => instance.endpoint)).toEqual([
			"http://127.0.0.1:4200",
		]);
	});

	it("rejects malformed health instead of trusting object shape", async () => {
		vi.stubGlobal(
			"fetch",
			vi
				.fn()
				.mockResolvedValue(
					new Response(JSON.stringify({ development: "yes" }), { status: 200 }),
				),
		);

		const health = await Effect.runPromise(
			DiscoverForgeHealth.pipe(Effect.provide(ForgeEndpointStoreLive)),
		);
		expect(Option.isNone(health)).toBe(true);
	});
});
