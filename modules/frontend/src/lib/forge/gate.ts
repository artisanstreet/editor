import type {
	ArtisanClientError,
	ArtisanClientErrorCode,
	ArtisanConnectionState,
} from "@artisan/transport/client";
import { Data } from "effect";

import {
	ArtisanErrorCode,
	format_artisan_error,
	type ArtisanErrorCode as ArtisanVisibleErrorCode,
} from "../errors/artisan-error-code";

export type ForgeHydrationOperation = "projects" | "session_defaults" | "threads";
export type ForgePairingGuidance = "none" | "possible" | "required";

export class ForgeHydrationFailure extends Data.TaggedError("ForgeHydrationFailure")<{
	readonly error: ArtisanClientError;
	readonly operation: ForgeHydrationOperation;
}> {}

export interface ForgeGateFailure {
	readonly code: ArtisanVisibleErrorCode;
	readonly diagnostics: {
		readonly attempts?: number;
		readonly client_code: ArtisanClientErrorCode;
		readonly protocol_code?: string;
		readonly retryable: boolean;
	};
	readonly message: string;
}

export type ForgeGatePhase =
	| { readonly phase: "connecting" }
	| { readonly phase: "reconnecting" }
	| { readonly generation: number; readonly phase: "hydrating" }
	| { readonly phase: "ready" }
	| {
			readonly attempts: number;
			readonly failure: ForgeGateFailure;
			readonly phase: "exhausted";
	  }
	| {
			readonly failure: ForgeGateFailure;
			readonly generation: number;
			readonly phase: "hydration-failed";
	  };

export interface ForgeGateModel {
	readonly dismissed: boolean;
	readonly has_hydrated_shell: boolean;
	readonly hydration_generation: number;
	readonly state: ForgeGatePhase;
}

interface ForgeGatePresentationBase {
	readonly description: string;
	readonly dismissible: boolean;
	readonly retry: "connection" | "hydration" | undefined;
	readonly show_start: boolean;
	readonly title: string;
}

export type ForgeGatePresentation =
	| (ForgeGatePresentationBase & {
			readonly failure: ForgeGateFailure;
			readonly tone: "error";
	  })
	| (ForgeGatePresentationBase & {
			readonly failure: undefined;
			readonly tone: "progress";
	  });

const client_error_codes: Readonly<Record<ArtisanClientErrorCode, ArtisanVisibleErrorCode>> = {
	configuration: ArtisanErrorCode.CLIENT_CONFIGURATION,
	connection: ArtisanErrorCode.CONNECTION_UNAVAILABLE,
	correlation_conflict: ArtisanErrorCode.CLIENT_STATE_FAILURE,
	disposed: ArtisanErrorCode.CLIENT_DISPOSED,
	event_overflow: ArtisanErrorCode.CLIENT_CAPACITY_EXCEEDED,
	malformed: ArtisanErrorCode.TRANSPORT_MALFORMED,
	protocol: ArtisanErrorCode.PROTOCOL_FAILURE,
	stream_closed: ArtisanErrorCode.CLIENT_STATE_FAILURE,
	stream_gap: ArtisanErrorCode.CLIENT_STATE_FAILURE,
	stream_not_found: ArtisanErrorCode.CLIENT_STATE_FAILURE,
	stream_overflow: ArtisanErrorCode.CLIENT_CAPACITY_EXCEEDED,
};

const pairing_protocol_codes = new Set(["invalid_pairing_code", "pairing_required"]);

const hydration_error_codes: Readonly<Record<ForgeHydrationOperation, ArtisanVisibleErrorCode>> = {
	projects: ArtisanErrorCode.HYDRATION_PROJECTS_FAILED,
	session_defaults: ArtisanErrorCode.HYDRATION_SESSION_DEFAULTS_FAILED,
	threads: ArtisanErrorCode.HYDRATION_THREADS_FAILED,
};

/** Maps every transport error to the stable safe code that Artisan presents. */
export const ForgeVisibleErrorCode = (error: ArtisanClientError): ArtisanVisibleErrorCode =>
	pairing_protocol_codes.has(error.protocol_code)
		? ArtisanErrorCode.PAIRING_REQUIRED
		: client_error_codes[error.code];

const make_forge_gate_failure = (
	code: ArtisanVisibleErrorCode,
	error: ArtisanClientError,
	attempts?: number,
): ForgeGateFailure => ({
	code,
	diagnostics: {
		...(attempts === undefined ? {} : { attempts }),
		client_code: error.code,
		...(error.protocol_code.length === 0 ? {} : { protocol_code: error.protocol_code }),
		retryable: error.retryable,
	},
	message: format_artisan_error(code),
});

export const InitialForgeGateModel: ForgeGateModel = {
	dismissed: false,
	has_hydrated_shell: false,
	hydration_generation: 0,
	state: { phase: "connecting" },
};

/**
 * Only a settled failure can be dismissed. The progress phases resolve on
 * their own within seconds, so offering to dismiss them would just race the
 * outcome the gate is about to report.
 */
export const IsForgeGateDismissible = (model: ForgeGateModel): boolean =>
	model.state.phase === "exhausted" || model.state.phase === "hydration-failed";

/**
 * Dismissal trades the remedy screen for the shell itself: the client stays
 * disconnected, so nothing loads and no command lands, but every surface,
 * control, and route remains open to inspection. It lasts for the current
 * outage only — a completed hydration clears it so the next disconnection is
 * reported again rather than silently swallowed.
 */
export const DismissForgeGate = (model: ForgeGateModel): ForgeGateModel =>
	IsForgeGateDismissible(model) ? { ...model, dismissed: true } : model;

/**
 * The live shell replaces the loading skeleton once it has hydrated at least
 * once, and also once the gate is dismissed — a dismissed gate exists to put
 * the real surfaces on screen, empty as they are.
 */
export const ForgeShellIsMounted = (model: ForgeGateModel): boolean =>
	model.has_hydrated_shell || model.dismissed;

/**
 * Transient reconnect and rehydration work stays behind a shell that has
 * already proved it can render. First hydration and settled failures still
 * need the gate because they have no usable live surface or require a remedy.
 */
export const ForgeGateIsVisible = (model: ForgeGateModel): boolean =>
	!model.dismissed &&
	model.state.phase !== "ready" &&
	(!model.has_hydrated_shell || IsForgeGateDismissible(model));

/** Input stays blocked while the gate is on screen, and only while it is. */
export const ForgeShellIsBlocked = (model: ForgeGateModel): boolean => ForgeGateIsVisible(model);

export const BeginForgeHydration = (model: ForgeGateModel): ForgeGateModel => {
	const generation = model.hydration_generation + 1;
	return {
		...model,
		hydration_generation: generation,
		state: { generation, phase: "hydrating" },
	};
};

/** A hydration result may mutate shell state only while its own generation still owns the gate. */
export const IsCurrentForgeHydration = (model: ForgeGateModel, generation: number): boolean =>
	model.state.phase === "hydrating" && model.state.generation === generation;

export const ObserveForgeConnection = (
	model: ForgeGateModel,
	state: ArtisanConnectionState,
): ForgeGateModel => {
	switch (state.phase) {
		case "ready":
			return BeginForgeHydration(model);
		case "connecting":
		case "reconnecting":
			return { ...model, state: { phase: state.phase } };
		case "exhausted":
			return {
				...model,
				state: {
					attempts: state.attempts,
					failure: make_forge_gate_failure(
						ForgeVisibleErrorCode(state.error),
						state.error,
						state.attempts,
					),
					phase: "exhausted",
				},
			};
	}
};

export const CompleteForgeHydration = (model: ForgeGateModel, generation: number): ForgeGateModel =>
	IsCurrentForgeHydration(model, generation)
		? {
				...model,
				dismissed: false,
				has_hydrated_shell: true,
				state: { phase: "ready" },
			}
		: model;

export const FailForgeHydration = (
	model: ForgeGateModel,
	generation: number,
	operation: ForgeHydrationOperation,
	error: ArtisanClientError,
): ForgeGateModel =>
	IsCurrentForgeHydration(model, generation)
		? {
				...model,
				state: {
					failure: make_forge_gate_failure(hydration_error_codes[operation], error),
					generation,
					phase: "hydration-failed",
				},
			}
		: model;

export const PresentForgeGate = (model: ForgeGateModel): ForgeGatePresentation => {
	const dismissible = IsForgeGateDismissible(model);
	switch (model.state.phase) {
		case "connecting":
			return {
				description: "Establishing a secure local session.",
				dismissible,
				failure: undefined,
				retry: undefined,
				show_start: false,
				title: "Connecting to Forge\u2026",
				tone: "progress",
			};
		case "reconnecting":
			return {
				description: "Your workspace stays in place while the connection is restored.",
				dismissible,
				failure: undefined,
				retry: undefined,
				show_start: false,
				title: "Reconnecting to Forge\u2026",
				tone: "progress",
			};
		case "hydrating":
			return {
				description: "Forge is syncing projects, threads, and runtime capabilities.",
				dismissible,
				failure: undefined,
				retry: undefined,
				show_start: false,
				title: "Loading your workspace\u2026",
				tone: "progress",
			};
		case "exhausted":
			const is_connection_unavailable =
				model.state.failure.code === ArtisanErrorCode.CONNECTION_UNAVAILABLE;

			return {
				description: is_connection_unavailable
					? "Start the installed local service, then reconnect."
					: model.state.failure.message,
				dismissible,
				failure: model.state.failure,
				retry: "connection",
				show_start: is_connection_unavailable,
				title: is_connection_unavailable ? "Forge is offline" : "Forge needs attention",
				tone: "error",
			};
		case "hydration-failed":
			return {
				description: model.state.failure.message,
				dismissible,
				failure: model.state.failure,
				retry: "hydration",
				show_start: false,
				title: "Could not load your workspace",
				tone: "error",
			};
		case "ready":
			return {
				description: "",
				dismissible,
				failure: undefined,
				retry: undefined,
				show_start: false,
				title: "",
				tone: "progress",
			};
	}
};

/** Distinguishes a proven pairing failure from a health-based recovery hint. */
export const PresentForgePairingGuidance = (
	presentation: ForgeGatePresentation,
	origin_reachable: boolean,
): ForgePairingGuidance => {
	if (presentation.tone !== "error") return "none";
	if (presentation.failure.code === ArtisanErrorCode.PAIRING_REQUIRED) return "required";

	return presentation.failure.code === ArtisanErrorCode.CONNECTION_UNAVAILABLE && origin_reachable
		? "possible"
		: "none";
};
