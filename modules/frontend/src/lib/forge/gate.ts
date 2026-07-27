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
	readonly has_hydrated_shell: boolean;
	readonly hydration_generation: number;
	readonly state: ForgeGatePhase;
}

export interface ForgeGatePresentation {
	readonly description: string;
	readonly retry: "connection" | "hydration" | undefined;
	readonly show_start: boolean;
	readonly title: string;
	readonly tone: "error" | "progress";
}

export const InitialForgeGateModel: ForgeGateModel = {
	has_hydrated_shell: false,
	hydration_generation: 0,
	state: { phase: "connecting" },
};

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
	switch (model.state.phase) {
		case "connecting":
			return {
				description: "Establishing a secure local session.",
				retry: undefined,
				show_start: false,
				title: "Connecting to Forge\u2026",
				tone: "progress",
			};
		case "reconnecting":
			return {
				description: "Your workspace stays in place while the connection is restored.",
				retry: undefined,
				show_start: false,
				title: "Reconnecting to Forge\u2026",
				tone: "progress",
			};
		case "hydrating":
			return {
				description: "Forge is syncing projects, threads, and runtime capabilities.",
				retry: undefined,
				show_start: false,
				title: "Loading your workspace\u2026",
				tone: "progress",
			};
		case "exhausted":
			return {
				description: "Start the installed local service, then reconnect.",
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
				retry: "hydration",
				show_start: false,
				title: "Could not load your workspace",
				tone: "error",
			};
		case "ready":
			return {
				description: "",
				retry: undefined,
				show_start: false,
				title: "",
				tone: "progress",
			};
	}
};
