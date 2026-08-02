import { describe, expect, it } from "vitest";

import { ArtisanClientError } from "../../../modules/transport/src/client";
import { ArtisanErrorCode } from "../../../modules/frontend/src/lib/errors/artisan-error-code";
import {
	BeginForgeHydration,
	CompleteForgeHydration,
	DismissForgeGate,
	FailForgeHydration,
	ForgeShellIsBlocked,
	ForgeShellIsMounted,
	InitialForgeGateModel,
	ObserveForgeConnection,
	PresentForgeGate,
	PresentForgePairingGuidance,
	ForgeVisibleErrorCode,
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
		expect(FailForgeHydration(second, 1, "threads", ConnectionError)).toBe(second);
		expect(CompleteForgeHydration(second, 2)).toMatchObject({
			has_hydrated_shell: true,
			state: { phase: "ready" },
		});
	});

	it.each([
		["configuration", ArtisanErrorCode.CLIENT_CONFIGURATION],
		["connection", ArtisanErrorCode.CONNECTION_UNAVAILABLE],
		["correlation_conflict", ArtisanErrorCode.CLIENT_STATE_FAILURE],
		["disposed", ArtisanErrorCode.CLIENT_DISPOSED],
		["event_overflow", ArtisanErrorCode.CLIENT_CAPACITY_EXCEEDED],
		["malformed", ArtisanErrorCode.TRANSPORT_MALFORMED],
		["protocol", ArtisanErrorCode.PROTOCOL_FAILURE],
		["request_overflow", ArtisanErrorCode.CLIENT_CAPACITY_EXCEEDED],
		["stream_closed", ArtisanErrorCode.CLIENT_STATE_FAILURE],
		["stream_gap", ArtisanErrorCode.CLIENT_STATE_FAILURE],
		["stream_not_found", ArtisanErrorCode.CLIENT_STATE_FAILURE],
		["stream_overflow", ArtisanErrorCode.CLIENT_CAPACITY_EXCEEDED],
		["subscription_overflow", ArtisanErrorCode.CLIENT_CAPACITY_EXCEEDED],
	] as const)("maps %s to %s", (code, expected) => {
		const error = new ArtisanClientError({
			cause: new Error("private transport cause"),
			code,
			message: "Unsafe transport detail.",
			protocol_code: "",
			retryable: false,
		});

		expect(ForgeVisibleErrorCode(error)).toBe(expected);
	});

	it("identifies pairing from its protocol code", () => {
		const pairing_error = new ArtisanClientError({
			...ConnectionError,
			protocol_code: "pairing_required",
		});

		expect(ForgeVisibleErrorCode(pairing_error)).toBe(ArtisanErrorCode.PAIRING_REQUIRED);
	});

	it("keeps a reachable Forge connection failure distinct from proven pairing failure", () => {
		const connection_presentation = PresentForgeGate(
			ObserveForgeConnection(InitialForgeGateModel, {
				attempts: 5,
				error: ConnectionError,
				phase: "exhausted",
			}),
		);
		const pairing_error = new ArtisanClientError({
			...ConnectionError,
			protocol_code: "pairing_required",
		});
		const pairing_presentation = PresentForgeGate(
			ObserveForgeConnection(InitialForgeGateModel, {
				attempts: 1,
				error: pairing_error,
				phase: "exhausted",
			}),
		);

		expect(ForgeVisibleErrorCode(ConnectionError)).toBe(
			ArtisanErrorCode.CONNECTION_UNAVAILABLE,
		);
		expect(PresentForgePairingGuidance(connection_presentation, true)).toBe("possible");
		expect(PresentForgePairingGuidance(connection_presentation, false)).toBe("none");
		expect(PresentForgePairingGuidance(pairing_presentation, false)).toBe("required");
	});

	it.each([
		["session_defaults", ArtisanErrorCode.HYDRATION_SESSION_DEFAULTS_FAILED],
		["projects", ArtisanErrorCode.HYDRATION_PROJECTS_FAILED],
		["threads", ArtisanErrorCode.HYDRATION_THREADS_FAILED],
	] as const)("uses the stable code for failed %s hydration", (operation, code) => {
		const hydrating = BeginForgeHydration(InitialForgeGateModel);
		const failed = FailForgeHydration(
			hydrating,
			hydrating.hydration_generation,
			operation,
			ConnectionError,
		);

		expect(PresentForgeGate(failed)).toMatchObject({
			failure: {
				code,
				diagnostics: {
					client_code: "connection",
					protocol_code: "transport.connection",
					retryable: true,
				},
			},
			retry: "hydration",
			show_start: false,
			tone: "error",
		});
	});

	it("keeps safe exhausted diagnostics and never presents raw transport text", () => {
		const exhausted = ObserveForgeConnection(InitialForgeGateModel, {
			attempts: 5,
			error: ConnectionError,
			phase: "exhausted",
		});
		const presentation = PresentForgeGate(exhausted);

		expect(presentation).toMatchObject({
			failure: {
				code: ArtisanErrorCode.CONNECTION_UNAVAILABLE,
				diagnostics: {
					attempts: 5,
					client_code: "connection",
					protocol_code: "transport.connection",
					retryable: true,
				},
			},
		});
		expect(JSON.stringify(presentation)).not.toContain("Transport bootstrap failed.");
	});

	it("omits absent protocol diagnostics and never attaches failures to progress", () => {
		const configuration_error = new ArtisanClientError({
			cause: new Error("private configuration cause"),
			code: "configuration",
			message: "Unsafe configuration detail.",
			protocol_code: "",
			retryable: false,
		});
		const exhausted = ObserveForgeConnection(InitialForgeGateModel, {
			attempts: 1,
			error: configuration_error,
			phase: "exhausted",
		});

		expect(PresentForgeGate(InitialForgeGateModel)).toMatchObject({
			failure: undefined,
			tone: "progress",
		});
		const presentation = PresentForgeGate(exhausted);

		expect(presentation).toMatchObject({
			failure: {
				diagnostics: {},
			},
			show_start: false,
			tone: "error",
		});

		if (presentation.tone === "progress") {
			throw new Error("Expected an error presentation.");
		}

		expect(presentation.failure.diagnostics).not.toHaveProperty("protocol_code");
	});

	it("offers dismissal only once a failure has settled", () => {
		const connecting = InitialForgeGateModel;
		const hydrating = BeginForgeHydration(connecting);
		const exhausted = ObserveForgeConnection(connecting, {
			attempts: 5,
			error: ConnectionError,
			phase: "exhausted",
		});

		expect(PresentForgeGate(connecting).dismissible).toBe(false);
		expect(PresentForgeGate(hydrating).dismissible).toBe(false);
		expect(PresentForgeGate(exhausted).dismissible).toBe(true);
		expect(DismissForgeGate(connecting)).toBe(connecting);
		expect(DismissForgeGate(hydrating)).toBe(hydrating);
		expect(DismissForgeGate(exhausted).dismissed).toBe(true);
	});

	it("hands the disconnected shell over when the gate is dismissed", () => {
		const exhausted = ObserveForgeConnection(InitialForgeGateModel, {
			attempts: 5,
			error: ConnectionError,
			phase: "exhausted",
		});
		const dismissed = DismissForgeGate(exhausted);

		expect(ForgeShellIsMounted(exhausted)).toBe(false);
		expect(ForgeShellIsBlocked(exhausted)).toBe(true);
		expect(ForgeShellIsMounted(dismissed)).toBe(true);
		expect(ForgeShellIsBlocked(dismissed)).toBe(false);
	});

	it("re-arms the gate for the next outage once a hydration succeeds", () => {
		const dismissed = DismissForgeGate(
			ObserveForgeConnection(InitialForgeGateModel, {
				attempts: 5,
				error: ConnectionError,
				phase: "exhausted",
			}),
		);
		const hydrating = ObserveForgeConnection(dismissed, { phase: "ready" });
		const ready = CompleteForgeHydration(hydrating, hydrating.hydration_generation);
		const offline_again = ObserveForgeConnection(ready, {
			attempts: 5,
			error: ConnectionError,
			phase: "exhausted",
		});

		/** The dismissal outlives the reconnect so the shell never flashes back to the gate. */
		expect(hydrating.dismissed).toBe(true);
		expect(ForgeShellIsBlocked(hydrating)).toBe(false);
		expect(ready.dismissed).toBe(false);
		expect(offline_again.dismissed).toBe(false);
		expect(ForgeShellIsBlocked(offline_again)).toBe(true);
	});
});
