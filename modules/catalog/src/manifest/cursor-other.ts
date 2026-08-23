import type { ModelDefinition } from "../schema";
import { cursor_native_speed, exceptional, standard } from "./options";

export const cursor_other_models = [
	{
		id: "cursor-gemini-3-7-flash",
		name: "Gemini 3.7 Flash",
		native_model_id: "gemini-3.7-flash",
		description:
			"Google's latest speed-tier model for high-throughput coding and large-context agent work.",
		harness: "cursor",
		provider: "google",
		routing: { kind: "default" },
		status: "prototype",
		capabilities: {
			thinking: {
				availability: "supported",
				default: "medium",
				options: [
					standard("light", "minimal"),
					standard("medium", "medium"),
					standard("high", "high"),
				],
			},
			speed_options: [cursor_native_speed("Gemini 3.7 Flash", false)],
			image_input: false,
			local_tools: true,
			mcp: true,
			web_search: false,
		},
	},
	{
		id: "cursor-gemini-3-1-pro",
		name: "Gemini 3.1 Pro",
		native_model_id: "gemini-3.1-pro",
		description: "Google's Pro-tier model, strongest on deep reasoning tasks.",
		harness: "cursor",
		provider: "google",
		routing: { kind: "default" },
		status: "prototype",
		capabilities: {
			thinking: {
				availability: "supported",
				default: "high",
				options: [
					standard("light", "minimal"),
					standard("medium", "medium"),
					standard("high", "high"),
				],
			},
			speed_options: [cursor_native_speed("Gemini 3.1 Pro", false)],
			image_input: false,
			local_tools: true,
			mcp: true,
			web_search: false,
		},
	},
	{
		id: "cursor-kimi-k3",
		name: "Kimi K3",
		native_model_id: "kimi-k3",
		description:
			"Open-weight frontier model; leads open models on long autonomous engineering work.",
		harness: "cursor",
		provider: "moonshot",
		routing: { kind: "default" },
		status: "prototype",
		capabilities: {
			/** Kimi documents reasoning_effort low/high/max; Kimi Code defaults to high. */
			thinking: {
				availability: "supported",
				default: "high",
				options: [
					standard("light", "low"),
					standard("high", "high"),
					exceptional("max", "max"),
				],
			},
			/** Kimi publishes its raw thinking rather than a written summary of it. */
			reasoning_display: "trace",
			speed_options: [cursor_native_speed("Kimi K3", false)],
			image_input: false,
			local_tools: true,
			mcp: true,
			web_search: false,
		},
	},
	{
		id: "cursor-glm-5-2",
		name: "GLM 5.2",
		native_model_id: "glm-5.2",
		description:
			"Z.ai's cost-efficient model for bounded coding, terminal work, and algorithmic reasoning.",
		harness: "cursor",
		provider: "zai",
		routing: { kind: "default" },
		status: "prototype",
		capabilities: {
			/** GLM 5.2 documents non-thinking plus High and Max thinking efforts. */
			thinking: {
				availability: "supported",
				default: "high",
				options: [
					standard("light", "none"),
					standard("high", "high"),
					exceptional("max", "max"),
				],
			},
			/** GLM streams raw chain-of-thought, with no summary layer over it. */
			reasoning_display: "trace",
			speed_options: [cursor_native_speed("GLM 5.2", false)],
			image_input: false,
			local_tools: true,
			mcp: true,
			web_search: false,
		},
	},
] satisfies ReadonlyArray<ModelDefinition>;
