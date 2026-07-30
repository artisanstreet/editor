import type { ArtisanConnectionState } from "@artisan/transport/client";

export type ForgeGatePhase =
	| { readonly phase: "connecting" }
	| { readonly phase: "reconnecting" }
	| { readonly generation: number; readonly phase: "hydrating" }
	| { readonly phase: "ready" }
	| {
			readonly attempts: number;
			readonly error: string;
			readonly phase: "exhausted";
	  }
	| {
			readonly error: string;
			readonly generation: number;
			readonly phase: "hydration-failed";
	  };

export interface ForgeGateModel {
	readonly dismissed: boolean;
	readonly has_hydrated_shell: boolean;
	readonly hydration_generation: number;
	readonly state: ForgeGatePhase;
}

export interface ForgeGatePresentation {
	readonly description: string;
	readonly dismissible: boolean;
	readonly retry: "connection" | "hydration" | undefined;
	readonly show_start: boolean;
	readonly title: string;
	readonly tone: "error" | "progress";
}

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

/** Input stays blocked while the gate is on screen, and only while it is. */
export const ForgeShellIsBlocked = (model: ForgeGateModel): boolean =>
	model.state.phase !== "ready" && !model.dismissed;

export const BeginForgeHydration = (model: ForgeGateModel): ForgeGateModel => {
	const generation = model.hydration_generation + 1;
	return {
		...model,
		hydration_generation: generation,
		state: { generation, phase: "hydrating" },
	};
};

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
					error: state.error.message,
					phase: "exhausted",
				},
			};
	}
};

export const CompleteForgeHydration = (model: ForgeGateModel, generation: number): ForgeGateModel =>
	model.state.phase === "hydrating" && model.state.generation === generation
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
	error: string,
): ForgeGateModel =>
	model.state.phase === "hydrating" && model.state.generation === generation
		? {
				...model,
				state: { error, generation, phase: "hydration-failed" },
			}
		: model;

export const PresentForgeGate = (model: ForgeGateModel): ForgeGatePresentation => {
	const dismissible = IsForgeGateDismissible(model);
	switch (model.state.phase) {
		case "connecting":
			return {
				description: "Establishing a secure local session.",
				dismissible,
				retry: undefined,
				show_start: false,
				title: "Connecting to Forge\u2026",
				tone: "progress",
			};
		case "reconnecting":
			return {
				description: "Your workspace stays in place while the connection is restored.",
				dismissible,
				retry: undefined,
				show_start: false,
				title: "Reconnecting to Forge\u2026",
				tone: "progress",
			};
		case "hydrating":
			return {
				description: "Forge is syncing projects, threads, and runtime capabilities.",
				dismissible,
				retry: undefined,
				show_start: false,
				title: "Loading your workspace\u2026",
				tone: "progress",
			};
		case "exhausted":
			return {
				description: "Start the installed local service, then reconnect.",
				dismissible,
				retry: "connection",
				show_start: true,
				title: "Forge is offline",
				tone: "error",
			};
		case "hydration-failed":
			return {
				description:
					model.state.error.length > 0
						? model.state.error
						: "Forge connected, but the workspace could not be loaded.",
				dismissible,
				retry: "hydration",
				show_start: false,
				title: "Could not load your workspace",
				tone: "error",
			};
		case "ready":
			return {
				description: "",
				dismissible,
				retry: undefined,
				show_start: false,
				title: "",
				tone: "progress",
			};
	}
};
