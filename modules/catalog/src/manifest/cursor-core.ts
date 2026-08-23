import type { ModelDefinition } from "../schema";
import {
	cursor_fast_speed,
	cursor_grok_fast_speed,
	cursor_grok_standard_speed,
	cursor_native_speed,
	cursor_standard_speed,
	standard,
} from "./options";

export const cursor_core_models = [
	{
		id: "cursor-composer-2-5",
		name: "Composer 2.5",
		native_model_id: "composer-2.5",
		description: "Cursor's own model, trained to be highly capable for agentic coding.",
		harness: "cursor",
		provider: "cursor",
		routing: { kind: "default" },
		status: "prototype",
		capabilities: {
			thinking: {
				availability: "native",
				description:
					"Composer 2.5 adapts its reasoning internally. Cursor CLI documents model selection but no explicit reasoning-effort control.",
			},
			speed_options: [
				cursor_standard_speed("Composer 2.5"),
				cursor_fast_speed("Composer 2.5"),
			],
			image_input: false,
			local_tools: true,
			mcp: true,
			web_search: false,
		},
	},
	{
		id: "cursor-auto",
		name: "Auto",
		native_model_id: "auto",
		description: "Analyzes each request and routes it to the right model for the job.",
		harness: "cursor",
		provider: "cursor",
		routing: { kind: "default" },
		status: "prototype",
		capabilities: {
			thinking: {
				availability: "native",
				description:
					"Cursor Router selects the underlying model and offers Cost, Balance, and Intelligence optimization modes. The selected model and effort can vary per request.",
			},
			speed_options: [cursor_native_speed("Auto", false)],
			image_input: false,
			local_tools: true,
			mcp: true,
			web_search: false,
		},
	},
	{
		id: "cursor-grok-4-6",
		name: "Grok 4.6",
		native_model_id: "cursor-grok-4.6",
		description:
			"A frontier model from Cursor and SpaceXAI for long-running agents and ambitious interactive work.",
		harness: "cursor",
		provider: "cursor",
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
					standard("xhigh", "xhigh"),
				],
			},
			speed_options: [
				cursor_grok_standard_speed("Grok 4.6"),
				cursor_grok_fast_speed("Grok 4.6", 2),
			],
			image_input: false,
			local_tools: true,
			mcp: true,
			web_search: false,
		},
	},
	{
		id: "cursor-grok-4-5",
		name: "Grok 4.5",
		native_model_id: "cursor-grok-4.5",
		description:
			"Jointly trained by Cursor and SpaceXAI for long-running coding and knowledge work.",
		harness: "cursor",
		provider: "cursor",
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
			speed_options: [
				cursor_grok_standard_speed("Grok 4.5"),
				cursor_grok_fast_speed("Grok 4.5"),
			],
			image_input: false,
			local_tools: true,
			mcp: true,
			web_search: false,
		},
	},
] satisfies ReadonlyArray<ModelDefinition>;
