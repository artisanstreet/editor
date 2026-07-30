import type { ModelDefinition } from "../schema";
import { standard, xai_standard_speed } from "./options";

export const xai_models = [
	{
		id: "grok-4-5",
		name: "Grok 4.5",
		native_model_id: "grok-4.5",
		description: "SpaceXAI's new frontier model.",
		harness: "grok",
		provider: "xai",
		routing: { kind: "default" },
		status: "prototype",
		capabilities: {
			thinking: {
				availability: "supported",
				default: "high",
				options: [
					standard("light", "low"),
					standard("medium", "medium"),
					standard("high", "high"),
				],
			},
			speed_options: [xai_standard_speed("Grok 4.5")],
			image_input: true,
			local_tools: true,
			mcp: false,
			web_search: true,
		},
	},
	{
		id: "grok-4-3",
		name: "Grok 4.3",
		native_model_id: "grok-4.3",
		harness: "grok",
		provider: "xai",
		routing: { kind: "default" },
		status: "prototype",
		capabilities: {
			thinking: {
				availability: "native",
				description:
					"Grok 4.3 reasons internally; Grok Build documents a reasoning-effort control only for Grok 4.5.",
			},
			speed_options: [xai_standard_speed("Grok 4.3")],
			image_input: true,
			local_tools: true,
			mcp: false,
			web_search: true,
		},
	},
	{
		id: "grok-build-0-1",
		name: "Grok Build 0.1",
		native_model_id: "grok-build-0.1",
		description: "Purpose-built coding model trained for agentic, multi-step workflows.",
		harness: "grok",
		provider: "xai",
		routing: { kind: "default" },
		status: "prototype",
		capabilities: {
			thinking: {
				availability: "native",
				description:
					"Grok Build 0.1 manages its reasoning internally; the CLI documents no effort control for it.",
			},
			speed_options: [xai_standard_speed("Grok Build 0.1")],
			image_input: false,
			local_tools: true,
			mcp: false,
			web_search: true,
		},
	},
	{
		id: "grok-composer-2-5",
		name: "Composer 2.5",
		native_model_id: "composer-2.5",
		description: "Cursor's own model, trained to be highly capable for agentic coding.",
		harness: "grok",
		provider: "cursor",
		routing: { kind: "default" },
		status: "prototype",
		capabilities: {
			thinking: {
				availability: "native",
				description:
					"Composer 2.5 adapts its reasoning internally. Grok Build selects it from the /model menu with no reasoning-effort control.",
			},
			speed_options: [xai_standard_speed("Composer 2.5")],
			image_input: false,
			local_tools: true,
			mcp: false,
			web_search: false,
		},
	},
] satisfies ReadonlyArray<ModelDefinition>;
