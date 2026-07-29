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
		input_consumption_multiplier: 1,
		output_consumption_multiplier: 1,
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
		input_consumption_multiplier: consumption_multiplier,
		output_consumption_multiplier: consumption_multiplier,
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
		input_consumption_multiplier: 1,
		output_consumption_multiplier: 1,
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
		input_consumption_multiplier: 2,
		output_consumption_multiplier: 2,
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
		input_consumption_multiplier: 1,
		output_consumption_multiplier: 1,
		default: true,
		description: `${model} uses 1x usage for 1x speed. Fast mode is not available in Grok Build.`,
		id: "standard",
		label: "Standard",
		native_value: "standard",
		source_url: "https://docs.x.ai/build/cli/reference",
		speed_multiplier: 1,
		verified_at: "2026-07-27",
	});

const cursor_standard_speed = (model: string) =>
	speed({
		availability: "always",
		consumption_basis: "standard",
		consumption_multiplier: 1,
		input_consumption_multiplier: 1,
		output_consumption_multiplier: 1,
		default: false,
		description: `${model} uses 1x token price for standard speed. Fast mode is available and enabled by default where the Cursor account supports it.`,
		id: "standard",
		label: "Standard",
		native_value: "standard",
		source_url: "https://cursor.com/changelog/composer-2-5",
		speed_multiplier: 1,
		verified_at: "2026-07-27",
	});

const cursor_fast_speed = (model: string) =>
	speed({
		availability: "dynamic",
		consumption_basis: "usage-credit-price",
		consumption_multiplier: 6,
		input_consumption_multiplier: 6,
		output_consumption_multiplier: 6,
		default: true,
		description: `${model} uses 6x token price for faster responses. Fast mode is available by default where the Cursor account supports it; Cursor has not published a numerical speed multiplier.`,
		id: "fast",
		label: "Fast",
		native_value: "fast",
		source_url: "https://cursor.com/changelog/composer-2-5",
		speed_multiplier: null,
		verified_at: "2026-07-27",
	});

const cursor_native_speed = (model: string, fast_available: boolean) =>
	speed({
		availability: "dynamic",
		consumption_basis: "usage-credit-price",
		consumption_multiplier: 1,
		input_consumption_multiplier: 1,
		output_consumption_multiplier: 1,
		default: true,
		description: `${model} uses its selected model's token price for provider-native speed. Fast mode is ${
			fast_available
				? "available in supported Cursor account configurations"
				: "not available as a separate configuration"
		}.`,
		id: "standard",
		label: "Native",
		native_value: "standard",
		source_url: "https://cursor.com/docs/models",
		speed_multiplier: 1,
		verified_at: "2026-07-27",
	});

const cursor_grok_standard_speed = (model: string) =>
	speed({
		availability: "always",
		consumption_basis: "usage-credit-price",
		consumption_multiplier: 1,
		input_consumption_multiplier: 1,
		output_consumption_multiplier: 1,
		default: true,
		description: `${model} uses 1x token price for standard speed. Fast mode is available in supported Cursor accounts.`,
		id: "standard",
		label: "Standard",
		native_value: "standard",
		source_url: "https://cursor.com/grok",
		speed_multiplier: 1,
		verified_at: "2026-07-27",
	});

const cursor_grok_fast_speed = (model: string) =>
	speed({
		availability: "dynamic",
		consumption_basis: "usage-credit-price",
		consumption_multiplier: null,
		input_consumption_multiplier: 2,
		output_consumption_multiplier: 3,
		default: false,
		description: `${model} uses 2x input token price and 3x output token price for faster responses. Fast mode is available where the Cursor account supports it; Cursor has not published a numerical speed multiplier.`,
		id: "fast",
		label: "Fast",
		native_value: "fast",
		source_url: "https://cursor.com/grok",
		speed_multiplier: null,
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

/** Claude Code selects the long-context variant via a native model-id suffix. */
const anthropic_context_window = {
	availability: "configurable",
	default: "standard",
	options: [
		{ id: "standard", label: "200K", native_suffix: "", tokens: 200000 },
		{ id: "extended", label: "1M", native_suffix: "[1m]", tokens: 1000000 },
	],
} as const;

/** Cursor hosts frontier models without exposing a separate reasoning-effort control. */
const cursor_hosted_thinking = {
	availability: "native",
	description:
		"Cursor manages reasoning for hosted frontier models; the CLI documents model choice without a separate effort control.",
} as const;

export const model_manifest = Schema.decodeUnknownSync(ModelManifest)({
	revision: "2026-07-29.2",
	providers: [
		{ id: "openai", label: "OpenAI" },
		{ id: "anthropic", label: "Anthropic" },
		{ id: "xai", label: "xAI" },
		{ id: "cursor", label: "Cursor" },
		{ id: "google", label: "Google" },
		{ id: "moonshot", label: "Moonshot" },
		{ id: "zai", label: "Z.ai" },
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
			label: "Claude",
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
		{
			id: "cursor",
			gateways: [],
			label: "Cursor Agent",
			permissions: {
				default: "supervised",
				options: [
					permission({
						approval_behavior: "prompts",
						availability: "always",
						description:
							"Use Cursor's interactive default and ask before terminal commands; configured deny rules remain authoritative.",
						edit_scope: "host",
						id: "supervised",
						label: "Supervised",
						native_value: "default",
						safety_boundary: "rules",
					}),
					permission({
						approval_behavior: "none",
						availability: "dynamic",
						description:
							"Run print mode with --force so commands and writes proceed without prompts; explicit deny rules still win.",
						edit_scope: "host",
						id: "unrestricted",
						label: "Force allow",
						native_value: "force",
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
			description: "Latest frontier agentic coding model.",
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
			description: "Balanced agentic coding model for everyday work.",
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
			description: "Fast and affordable agentic coding model.",
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
			description: "Frontier model for complex coding, research, and real-world work.",
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
			description: "Strong model for everyday coding.",
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
			description: "Small, fast, and cost-efficient model for simpler coding tasks.",
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
			description: "Ultra-fast coding model.",
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
			description: "Most capable for your hardest and longest-running tasks.",
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
				context_window: anthropic_context_window,
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
			description: "Best for everyday, complex tasks.",
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
				context_window: anthropic_context_window,
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
			description: "Efficient for routine tasks; recommended for most coding.",
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
				context_window: anthropic_context_window,
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
			description: "Fastest for quick answers at lower cost.",
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
			description:
				"Purpose-built coding model trained for agentic, multi-step workflows.",
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
			id: "cursor-grok-4-5",
			name: "Cursor Grok 4.5",
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
					cursor_grok_standard_speed("Cursor Grok 4.5"),
					cursor_grok_fast_speed("Cursor Grok 4.5"),
				],
				image_input: false,
				local_tools: true,
				mcp: true,
				web_search: false,
			},
		},
		{
			id: "cursor-gpt-5-6-sol",
			name: "GPT 5.6 Sol",
			native_model_id: "gpt-5.6-sol",
			description: "Latest frontier agentic coding model.",
			harness: "cursor",
			provider: "openai",
			routing: { kind: "default" },
			status: "prototype",
			capabilities: {
				thinking: cursor_hosted_thinking,
				speed_options: [cursor_native_speed("GPT 5.6 Sol", true)],
				image_input: false,
				local_tools: true,
				mcp: true,
				web_search: false,
			},
		},
		{
			id: "cursor-gpt-5-6-terra",
			name: "GPT 5.6 Terra",
			native_model_id: "gpt-5.6-terra",
			description: "Balanced agentic coding model for everyday work.",
			harness: "cursor",
			provider: "openai",
			routing: { kind: "default" },
			status: "prototype",
			capabilities: {
				thinking: cursor_hosted_thinking,
				speed_options: [cursor_native_speed("GPT 5.6 Terra", true)],
				image_input: false,
				local_tools: true,
				mcp: true,
				web_search: false,
			},
		},
		{
			id: "cursor-gpt-5-6-luna",
			name: "GPT 5.6 Luna",
			native_model_id: "gpt-5.6-luna",
			description: "Fast and affordable agentic coding model.",
			harness: "cursor",
			provider: "openai",
			routing: { kind: "default" },
			status: "prototype",
			capabilities: {
				thinking: cursor_hosted_thinking,
				speed_options: [cursor_native_speed("GPT 5.6 Luna", true)],
				image_input: false,
				local_tools: true,
				mcp: true,
				web_search: false,
			},
		},
		{
			id: "cursor-gpt-5-5",
			name: "GPT 5.5",
			native_model_id: "gpt-5.5",
			description: "Frontier model for complex coding, research, and real-world work.",
			harness: "cursor",
			provider: "openai",
			routing: { kind: "default" },
			status: "prototype",
			capabilities: {
				thinking: cursor_hosted_thinking,
				speed_options: [cursor_native_speed("GPT 5.5", true)],
				image_input: false,
				local_tools: true,
				mcp: true,
				web_search: false,
			},
		},
		{
			id: "cursor-gpt-5-4",
			name: "GPT 5.4",
			native_model_id: "gpt-5.4",
			description: "Strong model for everyday coding.",
			harness: "cursor",
			provider: "openai",
			routing: { kind: "default" },
			status: "prototype",
			capabilities: {
				thinking: cursor_hosted_thinking,
				speed_options: [cursor_native_speed("GPT 5.4", true)],
				image_input: false,
				local_tools: true,
				mcp: true,
				web_search: false,
			},
		},
		{
			id: "cursor-gpt-5-4-mini",
			name: "GPT 5.4 Mini",
			native_model_id: "gpt-5.4-mini",
			description: "Small, fast, and cost-efficient model for simpler coding tasks.",
			harness: "cursor",
			provider: "openai",
			routing: { kind: "default" },
			status: "prototype",
			capabilities: {
				thinking: cursor_hosted_thinking,
				speed_options: [cursor_native_speed("GPT 5.4 Mini", false)],
				image_input: false,
				local_tools: true,
				mcp: true,
				web_search: false,
			},
		},
		{
			id: "cursor-gpt-5-3-codex",
			name: "GPT 5.3 Codex",
			native_model_id: "gpt-5.3-codex",
			harness: "cursor",
			provider: "openai",
			routing: { kind: "default" },
			status: "prototype",
			capabilities: {
				thinking: cursor_hosted_thinking,
				speed_options: [cursor_native_speed("GPT 5.3 Codex", false)],
				image_input: false,
				local_tools: true,
				mcp: true,
				web_search: false,
			},
		},
		{
			id: "cursor-claude-fable-5",
			name: "Claude Fable 5",
			native_model_id: "claude-fable-5",
			description: "Most capable for your hardest and longest-running tasks.",
			harness: "cursor",
			provider: "anthropic",
			routing: { kind: "default" },
			status: "prototype",
			capabilities: {
				thinking: cursor_hosted_thinking,
				speed_options: [cursor_native_speed("Claude Fable 5", false)],
				image_input: false,
				local_tools: true,
				mcp: true,
				web_search: false,
			},
		},
		{
			id: "cursor-claude-opus-5",
			name: "Claude Opus 5",
			native_model_id: "claude-opus-5",
			description: "Best for everyday, complex tasks.",
			harness: "cursor",
			provider: "anthropic",
			routing: { kind: "default" },
			status: "prototype",
			capabilities: {
				thinking: cursor_hosted_thinking,
				speed_options: [cursor_native_speed("Claude Opus 5", true)],
				image_input: false,
				local_tools: true,
				mcp: true,
				web_search: false,
			},
		},
		{
			id: "cursor-claude-sonnet-5",
			name: "Claude Sonnet 5",
			native_model_id: "claude-sonnet-5",
			description: "Efficient for routine tasks; recommended for most coding.",
			harness: "cursor",
			provider: "anthropic",
			routing: { kind: "default" },
			status: "prototype",
			capabilities: {
				thinking: cursor_hosted_thinking,
				speed_options: [cursor_native_speed("Claude Sonnet 5", false)],
				image_input: false,
				local_tools: true,
				mcp: true,
				web_search: false,
			},
		},
		{
			id: "cursor-claude-haiku-4-5",
			name: "Claude Haiku 4.5",
			native_model_id: "claude-haiku-4-5",
			description: "Fastest for quick answers at lower cost.",
			harness: "cursor",
			provider: "anthropic",
			routing: { kind: "default" },
			status: "prototype",
			capabilities: {
				thinking: unavailable,
				speed_options: [cursor_native_speed("Claude Haiku 4.5", false)],
				image_input: false,
				local_tools: true,
				mcp: true,
				web_search: false,
			},
		},
		{
			id: "cursor-gemini-3-6-flash",
			name: "Gemini 3.6 Flash",
			native_model_id: "gemini-3.6-flash",
			harness: "cursor",
			provider: "google",
			routing: { kind: "default" },
			status: "prototype",
			capabilities: {
				thinking: cursor_hosted_thinking,
				speed_options: [cursor_native_speed("Gemini 3.6 Flash", false)],
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
			harness: "cursor",
			provider: "google",
			routing: { kind: "default" },
			status: "prototype",
			capabilities: {
				thinking: cursor_hosted_thinking,
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
			harness: "cursor",
			provider: "moonshot",
			routing: { kind: "default" },
			status: "prototype",
			capabilities: {
				thinking: cursor_hosted_thinking,
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
			harness: "cursor",
			provider: "zai",
			routing: { kind: "default" },
			status: "prototype",
			capabilities: {
				thinking: cursor_hosted_thinking,
				speed_options: [cursor_native_speed("GLM 5.2", false)],
				image_input: false,
				local_tools: true,
				mcp: true,
				web_search: false,
			},
		},
	],
});
