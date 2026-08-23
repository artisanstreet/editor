import { Context, Layer } from "effect";

import type { Engine, EngineDescriptor, EngineOpenInput } from "../engine";
import { MakeAcpEngine, type AcpEngineOptions } from "../acp/engine";

export const grok_transport = "grok-acp-stdio";

export const GrokEngineDescriptor: EngineDescriptor = {
	capabilities: {
		approval: { state: "supported", reason: "ACP permission requests are bridged to Artisan." },
		auth: { state: "supported", reason: "Uses Grok Build browser or API-key authentication." },
		cancel: { state: "supported" },
		close: { state: "supported" },
		events: { state: "supported" },
		global_guidance: {
			state: "unsupported",
			reason: "Grok Build discovers AGENTS.md and its own global instructions natively.",
		},
		model_catalog: {
			state: "unsupported",
			reason: "Grok models are supplied by Artisan's curated catalog.",
		},
		model_selection: {
			state: "supported",
			reason: "The selected model is passed to Grok ACP.",
		},
		native_continuation: {
			state: "unsupported",
			reason: "ACP does not guarantee that a loaded session may change model identity.",
		},
		native_tools: {
			state: "supported",
			reason: "ACP tool calls and file locations are normalized.",
		},
		probe: { state: "supported" },
		question: {
			state: "supported",
			reason: "ACP elicitations are bridged to Artisan questions.",
		},
		raw_frames: { state: "supported" },
		resume: { state: "supported", reason: "ACP session/load reopens Grok sessions." },
		start: { state: "supported" },
		steer: {
			state: "experimental",
			reason: "A waiting ACP session accepts a follow-up; active prompts cannot be steered in place.",
		},
		subagents: {
			state: "experimental",
			reason: "Subagent work is visible through ACP tool activity; child transcripts are not separated yet.",
		},
	},
	display_name: "Grok Build",
	id: "grok",
	transport: grok_transport,
};

const string_option = (input: EngineOpenInput, key: string) => {
	const value = input.provider_options?.[key];
	return typeof value === "string" && value.length > 0 ? value : undefined;
};

export const GrokAcpArgs = (input: EngineOpenInput) => {
	const permission = string_option(input, "grok.permission_mode");
	const effort = string_option(input, "grok.reasoning_effort");
	return [
		"--no-auto-update",
		...(input.model === undefined ? [] : ["--model", input.model]),
		...(effort === undefined ? [] : ["--reasoning-effort", effort]),
		...(input.permission_policy?.write_access === false
			? ["--permission-mode", "plan"]
			: permission === "auto"
				? ["--permission-mode", permission]
				: permission === "always-approve"
					? ["--always-approve"]
					: []),
		"agent",
		"stdio",
	];
};

export class GrokEngine extends Context.Service<GrokEngine, Engine>()("Artisan/GrokEngine") {}

export type GrokEngineOptions = Omit<AcpEngineOptions, "definition"> & {
	readonly executable?: string;
};

export const make_grok_engine_layer = (options: GrokEngineOptions = {}) =>
	Layer.effect(
		GrokEngine,
		MakeAcpEngine({
			...options,
			definition: {
				AcpArgs: GrokAcpArgs,
				Authenticated: (output) => !/not authenticated|not logged in/i.test(output),
				AuthMethod: (initialize, environment) => {
					const available = new Set(
						(initialize.authMethods ?? []).map((method) => method.id),
					);
					if (environment.XAI_API_KEY && available.has("xai.api_key"))
						return "xai.api_key";
					return available.has("cached_token") ? "cached_token" : undefined;
				},
				auth_probe_args: ["--no-auto-update", "models"],
				descriptor: GrokEngineDescriptor,
				executable: options.executable ?? "grok",
				image_input: "embedded",
				Version: (output) =>
					/\bgrok\s+(\d+\.\d+\.\d+(?:-[0-9A-Za-z._-]+)?)/i.exec(output)?.[1],
			},
		}),
	);
