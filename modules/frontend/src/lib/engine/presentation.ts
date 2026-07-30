import {
	SvglClaudeAILogo,
	SvglCursorLogo,
	SvglGeminiLogo,
	SvglGrokLogo,
	SvglKimiLogo,
	SvglOpenAILogo,
} from "@selemondev/svgl-svelte";
import QuestionMark from "@tabler/icons-svelte/icons/question-mark";
import type { Component } from "svelte";
import { model_manifest } from "@artisan/catalog";

/** Presents one engine's provider mark. @since 0.7.0 */
export interface EngineMark {
	/** The provider's own accent, as a CSS color, for meters and other tinted chrome. */
	readonly accent: string;
	readonly icon: Component;
	/** Marks a single-color logo that must invert with the theme. */
	readonly monochrome: boolean;
	/** Renders via currentColor in muted foreground; for placeholder glyphs, not logos. */
	readonly muted?: boolean;
}

const engine_marks: Readonly<Record<string, EngineMark>> = {
	/** Claude clay. */
	claude: { accent: "#d97757", icon: SvglClaudeAILogo, monochrome: false },
	/** OpenAI's sole chromatic accent; the marks themselves are monochrome. */
	codex: { accent: "#10a37f", icon: SvglOpenAILogo, monochrome: true },
	cursor: { accent: "#6b7280", icon: SvglCursorLogo, monochrome: true },
	grok: { accent: "#6b7280", icon: SvglGrokLogo, monochrome: true },
};

const unknown_engine_mark: EngineMark = {
	accent: "var(--primary)",
	icon: QuestionMark,
	monochrome: false,
	muted: true,
};

/** Resolves the provider mark for an engine, falling back to a neutral tool glyph. */
export const EngineMarkFor = (engine_id: string | undefined): EngineMark =>
	(engine_id === undefined ? undefined : engine_marks[engine_id]) ?? unknown_engine_mark;

const engine_names: Readonly<Record<string, string>> = {
	claude: "Claude",
	codex: "Codex",
	cursor: "Cursor",
	grok: "Grok",
};

/**
 * Names an engine without a round trip to the backend descriptor; an unknown
 * id wears its capitalized id and an absent id reads as unattributed work.
 */
export const EngineDisplayName = (engine_id: string | undefined): string => {
	if (engine_id === undefined) return "Other";
	return engine_names[engine_id] ?? engine_id.charAt(0).toUpperCase() + engine_id.slice(1);
};

/**
 * Marks for the lab that made a model, distinct from the engine serving it:
 * a Cursor-hosted GPT model carries the OpenAI mark, not the Cursor cube.
 * Anthropic and xAI wear their product marks (Claude, Grok) — that is what
 * people recognize, not the corporate logos.
 */
const provider_marks: Readonly<Record<string, EngineMark>> = {
	anthropic: { accent: "#d97757", icon: SvglClaudeAILogo, monochrome: false },
	cursor: { accent: "#6b7280", icon: SvglCursorLogo, monochrome: true },
	google: { accent: "#4285f4", icon: SvglGeminiLogo, monochrome: false },
	moonshot: { accent: "#6b7280", icon: SvglKimiLogo, monochrome: true },
	openai: { accent: "#10a37f", icon: SvglOpenAILogo, monochrome: true },
	xai: { accent: "#6b7280", icon: SvglGrokLogo, monochrome: true },
};

/** Resolves the mark for a model's lab, falling back to a neutral tool glyph. */
export const ProviderMarkFor = (provider_id: string | undefined): EngineMark =>
	(provider_id === undefined ? undefined : provider_marks[provider_id]) ?? unknown_engine_mark;

/** Names the Tailwind classes that size a provider mark and keep it theme-correct. */
export const EngineMarkClass = (mark: EngineMark, size = "size-5") => {
	const color = mark.muted === true ? " text-muted-foreground" : "";
	return mark.monochrome ? `${size} shrink-0 dark:invert${color}` : `${size} shrink-0${color}`;
};

/** Presents one usage slice: the model's catalog name and its lab's mark. */
export interface UsageSlicePresentation {
	readonly label: string;
	readonly mark: EngineMark;
}

/**
 * Resolves how a usage slice reads and which mark it wears. A catalog model
 * shows its name under its lab's mark (a Cursor-hosted GPT model wears the
 * OpenAI mark); a model the catalog no longer lists shows its raw id; a slice
 * without a model falls back to the engine's own name and mark.
 */
export const UsageSlicePresentationFor = (
	engine_id: string | undefined,
	model_id: string | undefined,
): UsageSlicePresentation => {
	const model =
		model_id === undefined
			? undefined
			: model_manifest.models.find(
					(candidate) =>
						candidate.native_model_id === model_id &&
						(engine_id === undefined || candidate.harness === engine_id),
				);
	if (model !== undefined) return { label: model.name, mark: ProviderMarkFor(model.provider) };
	return {
		label: model_id ?? EngineDisplayName(engine_id),
		mark: EngineMarkFor(engine_id),
	};
};
