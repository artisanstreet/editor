import { Effect } from "effect";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ForgeControl } from "../../modules/cli/src/lifecycle";
import { ForgeControlLive } from "../../modules/cli/src/node-control";
import { VerifyStoppedForgeProcess } from "../../modules/cli/src/node-launcher";
import type { ForgeRuntimeState } from "../../modules/cli/src/profile";

const State = (pid: number): ForgeRuntimeState => ({
	endpoint: "http://127.0.0.1:4848",
	instance_id: "2ef3d1c0-e8a4-4f4d-9d8a-744b1f18879d",
	pid,
	profile: "default",
	started_at: "2026-07-26T00:00:00.000Z",
	version: 1,
});

describe("Forge process identity fallback", () => {
	afterEach(() => vi.unstubAllGlobals());

	it("reclaims only a PID that is definitely absent", async () => {
		await expect(
			Effect.runPromise(VerifyStoppedForgeProcess(State(process.pid))),
		).rejects.toMatchObject({ code: "ownership" });
		await expect(
			Effect.runPromise(VerifyStoppedForgeProcess(State(2_147_483_647))),
		).resolves.toBeUndefined();
	});

	it("authenticates health and requires the exact Forge instance id", async () => {
		const fetch_mock = vi.fn(
			async () =>
				new Response(
					JSON.stringify({
						instance_id: State(1).instance_id,
						pid: 42,
						service: "artisan-forge",
						status: "ready",
						version: 1,
					}),
					{ headers: { "content-type": "application/json" }, status: 200 },
				),
		);
		vi.stubGlobal("fetch", fetch_mock);
		const control = await Effect.runPromise(
			ForgeControl.pipe(Effect.provide(ForgeControlLive)),
		);
		expect(
			await Effect.runPromise(
				control.Health("http://127.0.0.1:4848", State(1).instance_id, "a".repeat(43)),
			),
		).toBe(true);
		expect(
			await Effect.runPromise(
				control.Health(
					"http://127.0.0.1:4848",
					"11111111-1111-4111-8111-111111111111",
					"a".repeat(43),
				),
			),
		).toBe(false);
		expect(fetch_mock).toHaveBeenCalledWith(
			new URL("http://127.0.0.1:4848/api/control/status"),
			expect.objectContaining({
				headers: { Authorization: `Bearer ${"a".repeat(43)}` },
				method: "GET",
			}),
		);
	});
});
