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
	Schema.decodeUnknownSync(ThinkingOption)({ economics: "standard", id, native_value });

export const exceptional = (id: ThinkingOption["id"], native_value: string) =>
	Schema.decodeUnknownSync(ThinkingOption)({
		economics: "diminishing-returns",
		id,
		native_value,
	});

/** Claude Code selects the long-context variant via a native model-id suffix. */
export const anthropic_context_window = Schema.decodeUnknownSync(ContextWindowCapability)({
	availability: "configurable",
	default: "standard",
	options: [
		{
			description: "The default window. The session compacts once it fills.",
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
