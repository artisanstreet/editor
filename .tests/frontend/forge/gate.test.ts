import { describe, expect, it } from "vitest";

import { ArtisanClientError } from "../../../modules/transport/src/client-contract";
import {
	BeginForgeHydration,
	CompleteForgeHydration,
	FailForgeHydration,
	InitialForgeGateModel,
	ObserveForgeConnection,
	PresentForgeGate,
} from "../../../modules/frontend/src/lib/forge/gate";

const ConnectionError = new ArtisanClientError({
	cause: new Error("socket closed"),
	code: "connection",
	message: "Transport bootstrap failed.",
	protocol_code: "transport.connection",
	retryable: true,
});

describe("ForgeGate", () => {
	it("hydrates each ready connection with a new generation", () => {
		const first = ObserveForgeConnection(InitialForgeGateModel, { phase: "ready" });
		const reconnected = ObserveForgeConnection(
			ObserveForgeConnection(first, { phase: "reconnecting" }),
			{ phase: "ready" },
		);

		expect(first.state).toEqual({ generation: 1, phase: "hydrating" });
		expect(reconnected.state).toEqual({ generation: 2, phase: "hydrating" });
	});

	it("keeps the hydrated shell mounted through reconnects and failures", () => {
		const hydrating = BeginForgeHydration(InitialForgeGateModel);
		const ready = CompleteForgeHydration(hydrating, hydrating.hydration_generation);
		const reconnecting = ObserveForgeConnection(ready, { phase: "reconnecting" });
		const exhausted = ObserveForgeConnection(reconnecting, {
			attempts: 5,
			error: ConnectionError,
			phase: "exhausted",
		});

		expect(ready.has_hydrated_shell).toBe(true);
		expect(reconnecting.has_hydrated_shell).toBe(true);
		expect(exhausted.has_hydrated_shell).toBe(true);
		expect(PresentForgeGate(exhausted)).toMatchObject({
			retry: "connection",
			show_start: true,
			title: "Forge is offline",
			tone: "error",
		});
	});

	it("ignores stale hydration results after connection state advances", () => {
		const first = BeginForgeHydration(InitialForgeGateModel);
		const second = BeginForgeHydration(first);

		expect(CompleteForgeHydration(second, 1)).toBe(second);
		expect(FailForgeHydration(second, 1, "stale")).toBe(second);
		expect(CompleteForgeHydration(second, 2)).toMatchObject({
			has_hydrated_shell: true,
			state: { phase: "ready" },
		});
	});

	it("exposes a dedicated hydration retry without suggesting Forge restart", () => {
		const hydrating = BeginForgeHydration(InitialForgeGateModel);
		const failed = FailForgeHydration(
			hydrating,
			hydrating.hydration_generation,
			"Catalog unavailable",
		);

		expect(PresentForgeGate(failed)).toEqual({
			description: "Catalog unavailable",
			retry: "hydration",
			show_start: false,
			title: "Could not load your workspace",
			tone: "error",
		});
	});
});
