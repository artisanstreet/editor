import { Schema } from "effect";

import {
	PermissionOption,
	SpeedOption,
	ContextWindowCapability,
	ThinkingLevel,
	ThinkingOption,
} from "../schema";

export const thinking_level_labels = Schema.decodeUnknownSync(
	Schema.Record(ThinkingLevel, Schema.String),
)({
	light: "Light",
	medium: "Medium",
	high: "High",
	xhigh: "Extra High",
	max: "Max",
	ultra: "Ultra",
});

export const unavailable = Schema.decodeUnknownSync(
	Schema.Struct({ availability: Schema.Literal("unavailable") }),
)({ availability: "unavailable" });

const speed = (input: SpeedOption) => Schema.decodeUnknownSync(SpeedOption)(input);

export const openai_standard_speed = (model: string, fast_available: boolean) =>
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

export const openai_fast_speed = (model: string, consumption_multiplier: number) =>
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

export const anthropic_standard_speed = (model: string, fast_available: boolean) =>
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

export const anthropic_fast_speed = (model: string) =>
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

export const xai_standard_speed = (model: string) =>
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

export const cursor_standard_speed = (model: string) =>
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

export const cursor_fast_speed = (model: string) =>
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

export const cursor_native_speed = (model: string, fast_available: boolean) =>
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

export const cursor_grok_standard_speed = (model: string) =>
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

export const cursor_grok_fast_speed = (model: string) =>
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

export const permission = (input: PermissionOption) =>
	Schema.decodeUnknownSync(PermissionOption)(input);

export const standard = (id: ThinkingOption["id"], native_value: string) =>
	Schema.decodeUnknownSync(ThinkingOption)({
		economics: "standard",
		id,
		native_value,
		presentation_group: "base",
	});

export const exceptional = (id: ThinkingOption["id"], native_value: string) =>
	Schema.decodeUnknownSync(ThinkingOption)({
		economics: "diminishing-returns",
		id,
		native_value,
		presentation_group: "special",
	});

/**
 * Codex Ultra coordinates parallel subagents at the harness level rather than
 * selecting another step in the model's reasoning budget.
 *
 * Carries the same advisory as the extended context window, and for the same
 * reason: both multiply what a single turn costs without saying so anywhere
 * the reader would look. Ultra's multiplier is the subagent fan-out — each one
 * is a full model run with its own context, so a turn is billed as many, and
 * the count is decided by the harness rather than by the person choosing it.
 */
export const harness_orchestration = (id: "ultra", native_value: string) =>
	Schema.decodeUnknownSync(ThinkingOption)({
		advisory: "Not recommended.",
		description:
			"Ultra lets Codex coordinate multiple subagents in parallel and synthesize their results. Each subagent is a separate model run with its own context, so one Ultra turn can cost several times an ordinary one — and how many it spawns is Codex's decision, not yours. It earns that only when complex work splits cleanly into genuinely independent tasks; on work that does not split, it pays the fan-out and synthesizes very little.",
		economics: "harness-orchestration",
		id,
		native_value,
		presentation_group: "special",
	});

/**
 * Claude Code selects the long-context variant via a native model-id suffix.
 *
 * Extended is the default because on Claude 5 it is free: from Opus 4.6 onward
 * the full 1M window bills at the standard per-token rate, so a 900K prompt
 * costs the same per token as a 9K one. The premium that made 200K the prudent
 * default — $10/$37.50 per million past 200K on Opus 4.6 — no longer applies.
 * With no price to weigh against it, holding a session to a fifth of its window
 * only buys earlier compaction, and compaction is lossy.
 *
 * @see https://docs.anthropic.com/en/docs/about-claude/pricing
 * @see https://www.anthropic.com/news/claude-opus-5
 */
export const anthropic_context_window = Schema.decodeUnknownSync(ContextWindowCapability)({
	availability: "configurable",
	default: "extended",
	options: [
		{
			description: "A fifth of the window. The session compacts once it fills.",
			id: "standard",
			label: "200K",
			native_suffix: "",
			tokens: 200000,
		},
		{
			description: "5x the window at 1x the token price — no long-context premium past 200K.",
			id: "extended",
			label: "1M",
			native_suffix: "[1m]",
			tokens: 1000000,
		},
	],
});

/**
 * Codex takes its window as configuration, not as part of the model id.
 *
 * Every GPT-5 model resolves to 272K in Codex's own model cache, and the
 * larger window is reached by overriding `model_context_window` rather than by
 * naming a different model — so these options carry `native_config` and no
 * suffix. Codex then compacts at nine tenths of whatever it resolved, which is
 * how the extended option moves compaction from ~245K to ~945K.
 *
 * Standard is the default, and unlike Anthropic's the premium here is real:
 * input past 272K bills at roughly double, which is why the extended option
 * opens by saying so rather than by advertising the extra room.
 */
export const openai_context_window = Schema.decodeUnknownSync(ContextWindowCapability)({
	availability: "configurable",
	default: "standard",
	options: [
		{
			description: "What Codex resolves for every GPT-5 model. Compacts near 245K.",
			id: "standard",
			label: "272K",
			native_suffix: "",
			tokens: 272000,
		},
		{
			advisory: "Not recommended.",
			description:
				"Input past 272K bills at about twice the standard rate, and it is the whole conversation that is re-sent on every turn — so the premium applies again to the same tokens with each message, not once. Compaction moving to ~945K means far more context is carried into far more turns. Reach for it only when a task genuinely cannot be split, and expect a turn near the ceiling to cost multiples of the same turn under 272K.",
			id: "extended",
			label: "1M",
			native_config: { model_context_window: 1050000 },
			native_suffix: "1m",
			tokens: 1050000,
		},
	],
});
