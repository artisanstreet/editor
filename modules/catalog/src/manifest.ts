import { Schema } from "effect";

import {
	ModelManifest,
	PermissionOption,
	SpeedOption,
	ThinkingLevel,
	ThinkingOption,
} from "./schema";

export const thinking_level_labels = Schema.decodeUnknownSync(
	Schema.Record(ThinkingLevel, Schema.String),
)({
	light: "Light",
	medium: "Medium",
	high: "High",
	xhigh: "Extra High",
	max: "Max",
});

const unavailable = Schema.decodeUnknownSync(
	Schema.Struct({ availability: Schema.Literal("unavailable") }),
)({ availability: "unavailable" });

const speed = (input: SpeedOption) => Schema.decodeUnknownSync(SpeedOption)(input);

const openai_standard_speed = (model: string, fast_available: boolean) =>
	speed({
		availability: "always",
		consumption_basis: "standard",
		consumption_multiplier: 1,
		default: true,
		description: `${model} uses 1x ChatGPT credits for 1x speed. Fast mode is ${
			fast_available
				? "available on supported ChatGPT sessions"
				: "not available for this model"
		}; API-key sessions use API token pricing instead.`,
		id: "standard",
		label: "Standard",
		native_value: "standard",
		source_url: "https://learn.chatgpt.com/docs/agent-configuration/speed",
		speed_multiplier: 1,
		verified_at: "2026-07-27",
	});

const openai_fast_speed = (model: string, consumption_multiplier: number) =>
	speed({
		availability: "dynamic",
		consumption_basis: "chatgpt-credits",
		consumption_multiplier,
		default: false,
		description: `${model} uses ${consumption_multiplier}x ChatGPT credits for 1.5x speed. Fast mode is available when signed in with ChatGPT; API-key sessions use API token pricing instead.`,
		id: "fast",
		label: "Fast",
		native_value: "fast",
		source_url: "https://learn.chatgpt.com/docs/agent-configuration/speed",
		speed_multiplier: 1.5,
		verified_at: "2026-07-27",
	});

const anthropic_standard_speed = (model: string, fast_available: boolean) =>
	speed({
		availability: "always",
		consumption_basis: "standard",
		consumption_multiplier: 1,
		default: true,
		description: `${model} uses 1x token price for 1x speed. Fast mode is ${
			fast_available
				? "available with separately billed usage credits"
				: "not available for this model"
		}.`,
		id: "standard",
		label: "Standard",
		native_value: "standard",
		source_url: "https://code.claude.com/docs/en/fast-mode",
		speed_multiplier: 1,
		verified_at: "2026-07-27",
	});

const anthropic_fast_speed = (model: string) =>
	speed({
		availability: "dynamic",
		consumption_basis: "usage-credit-price",
		consumption_multiplier: 2,
		default: false,
		description: `${model} uses 2x token price for up to 2.5x speed. Fast mode is available for eligible sessions with usage credits and does not use included subscription limits.`,
		id: "fast",
		label: "Fast",
		native_value: "fast",
		source_url: "https://code.claude.com/docs/en/fast-mode",
		speed_multiplier: 2.5,
		verified_at: "2026-07-27",
	});

const xai_standard_speed = (model: string) =>
	speed({
		availability: "always",
		consumption_basis: "standard",
		consumption_multiplier: 1,
		default: true,
		description: `${model} uses 1x usage for 1x speed. Fast mode is not available in Grok Build.`,
		id: "standard",
		label: "Standard",
		native_value: "standard",
		source_url: "https://docs.x.ai/build/cli/reference",
		speed_multiplier: 1,
		verified_at: "2026-07-27",
	});

const permission = (input: PermissionOption) => Schema.decodeUnknownSync(PermissionOption)(input);

const standard = (id: ThinkingOption["id"], native_value: string) =>
	Schema.decodeUnknownSync(ThinkingOption)({ economics: "standard", id, native_value });

const exceptional = (id: ThinkingOption["id"], native_value: string) =>
	Schema.decodeUnknownSync(ThinkingOption)({
		economics: "diminishing-returns",
		id,
		native_value,
	});

export const model_manifest = Schema.decodeUnknownSync(ModelManifest)({
	revision: "2026-07-27.1",
	providers: [
		{ id: "openai", label: "OpenAI" },
		{ id: "anthropic", label: "Anthropic" },
		{ id: "xai", label: "xAI" },
	],
	harnesses: [
		{
			id: "codex",
			gateways: [],
			label: "Codex",
			permissions: {
				default: "supervised",
				options: [
					permission({
						approval_behavior: "none",
						availability: "always",
						description:
							"Inspect the workspace without writing files; Codex still applies its read-only sandbox.",
						edit_scope: "none",
						id: "restricted",
						label: "Read only",
						native_value: "read-only",
						safety_boundary: "sandbox",
					}),
					permission({
						approval_behavior: "prompts",
						availability: "always",
						description:
							"Write inside the workspace while Codex decides when an action needs approval.",
						edit_scope: "workspace",
						id: "supervised",
						label: "Supervised",
						native_value: "workspace-write",
						safety_boundary: "sandbox",
					}),
					permission({
						approval_behavior: "none",
						availability: "always",
						description:
							"Run without approval prompts while remaining inside Codex's workspace-write sandbox.",
						edit_scope: "workspace",
						id: "autonomous",
						label: "Auto approve",
						native_value: "workspace-write-no-prompts",
						safety_boundary: "sandbox",
					}),
				],
			},
		},
		{
			id: "claude",
			gateways: [],
			label: "Claude Code",
			permissions: {
				default: "supervised",
				options: [
					permission({
						approval_behavior: "prompts",
						availability: "always",
						description:
							"Explore and propose a plan without editing source files until the plan is approved.",
						edit_scope: "none",
						id: "restricted",
						label: "Plan only",
						native_value: "plan",
						safety_boundary: "plan",
					}),
					permission({
						approval_behavior: "prompts",
						availability: "always",
						description:
							"Auto-approve reads and ask before actions that require permission.",
						edit_scope: "host",
						id: "supervised",
						label: "Supervised",
						native_value: "default",
						safety_boundary: "rules",
					}),
					permission({
						approval_behavior: "prompts",
						availability: "always",
						description:
							"Auto-approve in-scope edits and common filesystem operations; prompt for other commands.",
						edit_scope: "host",
						id: "trusted",
						label: "Accept edits",
						native_value: "acceptEdits",
						safety_boundary: "rules",
					}),
					permission({
						approval_behavior: "classifier",
						availability: "dynamic",
						description:
							"Use Anthropic's classifier to run routine actions and block or escalate risky actions.",
						edit_scope: "host",
						id: "autonomous",
						label: "Auto",
						native_value: "auto",
						safety_boundary: "rules",
					}),
					permission({
						approval_behavior: "none",
						availability: "dynamic",
						description:
							"Disable ordinary permission prompts and safety checks; administrator policy may forbid this mode.",
						edit_scope: "host",
						id: "unrestricted",
						label: "Bypass permissions",
						native_value: "bypassPermissions",
						safety_boundary: "bypassed",
					}),
				],
			},
		},
		{
			id: "grok",
			gateways: [],
			label: "Grok Build",
			permissions: {
				default: "supervised",
				options: [
					permission({
						approval_behavior: "prompts",
						availability: "always",
						description:
							"Prompt for tool calls that are not already allowed by a rule.",
						edit_scope: "host",
						id: "supervised",
						label: "Supervised",
						native_value: "ask",
						safety_boundary: "rules",
					}),
					permission({
						approval_behavior: "classifier",
						availability: "dynamic",
						description:
							"Use xAI's classifier to approve safe tools while dangerous actions may still prompt.",
						edit_scope: "host",
						id: "autonomous",
						label: "Auto",
						native_value: "auto",
						safety_boundary: "rules",
					}),
					permission({
						approval_behavior: "none",
						availability: "always",
						description:
							"Auto-approve tool calls while deny rules and pre-tool hooks remain authoritative.",
						edit_scope: "host",
						id: "unrestricted",
						label: "Always approve",
						native_value: "always-approve",
						safety_boundary: "rules",
					}),
				],
			},
		},
	],
	models: [
		{
			id: "codex-sol",
			name: "GPT 5.6 Sol",
			native_model_id: "gpt-5.6-sol",
			harness: "codex",
			provider: "openai",
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
					openai_standard_speed("GPT 5.6 Sol", true),
					openai_fast_speed("GPT 5.6 Sol", 2.5),
				],
				image_input: true,
				local_tools: true,
				mcp: true,
				web_search: true,
			},
		},
		{
			id: "codex-terra",
			name: "GPT 5.6 Terra",
			native_model_id: "gpt-5.6-terra",
			harness: "codex",
			provider: "openai",
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
					openai_standard_speed("GPT 5.6 Terra", true),
					openai_fast_speed("GPT 5.6 Terra", 2.5),
				],
				image_input: true,
				local_tools: true,
				mcp: true,
				web_search: true,
			},
		},
		{
			id: "codex-luna",
			name: "GPT 5.6 Luna",
			native_model_id: "gpt-5.6-luna",
			harness: "codex",
			provider: "openai",
			routing: { kind: "default" },
			status: "prototype",
			capabilities: {
				thinking: {
					availability: "supported",
					default: "medium",
					options: [
						standard("light", "low"),
						standard("medium", "medium"),
						standard("high", "high"),
						standard("xhigh", "xhigh"),
					],
				},
				speed_options: [
					openai_standard_speed("GPT 5.6 Luna", true),
					openai_fast_speed("GPT 5.6 Luna", 2.5),
				],
				image_input: true,
				local_tools: true,
				mcp: true,
				web_search: true,
			},
		},
		{
			id: "codex-gpt-5-5",
			name: "GPT 5.5",
			native_model_id: "gpt-5.5",
			harness: "codex",
			provider: "openai",
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
					openai_standard_speed("GPT 5.5", true),
					openai_fast_speed("GPT 5.5", 2.5),
				],
				image_input: true,
				local_tools: true,
				mcp: true,
				web_search: true,
			},
		},
		{
			id: "codex-gpt-5-4",
			name: "GPT 5.4",
			native_model_id: "gpt-5.4",
			harness: "codex",
			provider: "openai",
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
					openai_standard_speed("GPT 5.4", true),
					openai_fast_speed("GPT 5.4", 2),
				],
				image_input: true,
				local_tools: true,
				mcp: true,
				web_search: true,
			},
		},
		{
			id: "codex-gpt-5-4-mini",
			name: "GPT 5.4 Mini",
			native_model_id: "gpt-5.4-mini",
			harness: "codex",
			provider: "openai",
			routing: { kind: "default" },
			status: "prototype",
			capabilities: {
				thinking: {
					availability: "supported",
					default: "medium",
					options: [
						standard("light", "low"),
						standard("medium", "medium"),
						standard("high", "high"),
						standard("xhigh", "xhigh"),
					],
				},
				speed_options: [openai_standard_speed("GPT 5.4 Mini", false)],
				image_input: true,
				local_tools: true,
				mcp: true,
				web_search: true,
			},
		},
		{
			id: "codex-spark",
			name: "GPT 5.3 Codex Spark",
			native_model_id: "gpt-5.3-codex-spark",
			harness: "codex",
			provider: "openai",
			routing: { kind: "default" },
			status: "prototype",
			capabilities: {
				thinking: {
					availability: "supported",
					default: "medium",
					options: [
						standard("light", "low"),
						standard("medium", "medium"),
						standard("high", "high"),
						standard("xhigh", "xhigh"),
					],
				},
				speed_options: [openai_standard_speed("GPT 5.3 Codex Spark", false)],
				image_input: false,
				local_tools: true,
				mcp: true,
				web_search: true,
			},
		},
		{
			id: "claude-fable",
			name: "Claude Fable 5",
			native_model_id: "claude-fable-5",
			harness: "claude",
			provider: "anthropic",
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
						exceptional("max", "max"),
					],
				},
				speed_options: [anthropic_standard_speed("Claude Fable 5", false)],
				image_input: true,
				local_tools: true,
				mcp: true,
				web_search: true,
			},
		},
		{
			id: "claude-opus",
			name: "Claude Opus 5",
			native_model_id: "claude-opus-5",
			harness: "claude",
			provider: "anthropic",
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
						exceptional("max", "max"),
					],
				},
				speed_options: [
					anthropic_standard_speed("Claude Opus 5", true),
					anthropic_fast_speed("Claude Opus 5"),
				],
				image_input: true,
				local_tools: true,
				mcp: true,
				web_search: true,
			},
		},
		{
			id: "claude-sonnet",
			name: "Claude Sonnet 5",
			native_model_id: "claude-sonnet-5",
			harness: "claude",
			provider: "anthropic",
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
						exceptional("max", "max"),
					],
				},
				speed_options: [anthropic_standard_speed("Claude Sonnet 5", false)],
				image_input: true,
				local_tools: true,
				mcp: true,
				web_search: true,
			},
		},
		{
			id: "claude-haiku",
			name: "Claude Haiku 4.5",
			native_model_id: "claude-haiku-4-5",
			harness: "claude",
			provider: "anthropic",
			routing: { kind: "default" },
			status: "prototype",
			capabilities: {
				thinking: unavailable,
				speed_options: [anthropic_standard_speed("Claude Haiku 4.5", false)],
				image_input: true,
				local_tools: true,
				mcp: true,
				web_search: true,
			},
		},
		{
			id: "grok-4-5",
			name: "Grok 4.5",
			native_model_id: "grok-4.5",
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
	],
});
