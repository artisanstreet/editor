import {
	SvglClaudeAILogo,
	SvglCursorLogo,
	SvglDeepSeekLogo,
	SvglGeminiLogo,
	SvglGrokLogo,
	SvglMetaLogo,
	SvglOpenAILogo,
	SvglQwenLogo,
} from "@selemondev/svgl-svelte";
import QuestionMark from "@tabler/icons-svelte/icons/question-mark";
import type { Component } from "svelte";
import { model_manifest } from "@artisan/catalog";
import KimiLogo from "$lib/assets/brands/kimi/logo.svelte";
import HermesLogo from "$lib/assets/brands/hermes/logo.svelte";
import MinimaxLogo from "$lib/assets/brands/minimax/logo.svelte";
import NvidiaLogo from "$lib/assets/brands/nvidia/logo.svelte";
import OpenCodeLogo from "$lib/assets/brands/opencode/logo.svelte";
import TencentLogo from "$lib/assets/brands/tencent/logo.svelte";
import XiaomiLogo from "$lib/assets/brands/xiaomi/logo.svelte";
import ZaiLogo from "$lib/assets/brands/zai/logo.svelte";

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
	hermes: { accent: "#8b5cf6", icon: HermesLogo, monochrome: true },
	opencode2: { accent: "#6b7280", icon: OpenCodeLogo, monochrome: false },
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
	hermes: "Hermes",
	opencode2: "OpenCode",
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
	deepseek: { accent: "#4d6bfe", icon: SvglDeepSeekLogo, monochrome: false },
	google: { accent: "#4285f4", icon: SvglGeminiLogo, monochrome: false },
	meta: { accent: "#0081fb", icon: SvglMetaLogo, monochrome: false },
	minimax: { accent: "#e76f51", icon: MinimaxLogo, monochrome: false },
	moonshot: { accent: "#6b7280", icon: KimiLogo, monochrome: false },
	nvidia: { accent: "#76b900", icon: NvidiaLogo, monochrome: false },
	opencode: { accent: "#6b7280", icon: OpenCodeLogo, monochrome: false },
	"opencode-go": { accent: "#6b7280", icon: OpenCodeLogo, monochrome: false },
	openai: { accent: "#10a37f", icon: SvglOpenAILogo, monochrome: true },
	"openai-codex": { accent: "#10a37f", icon: SvglOpenAILogo, monochrome: true },
	qwen: { accent: "#615ced", icon: SvglQwenLogo, monochrome: false },
	tencent: { accent: "#006cb6", icon: TencentLogo, monochrome: false },
	xai: { accent: "#6b7280", icon: SvglGrokLogo, monochrome: true },
	xiaomi: { accent: "#ff6900", icon: XiaomiLogo, monochrome: false },
	zai: { accent: "#6b7280", icon: ZaiLogo, monochrome: false },
	zhipu: { accent: "#6b7280", icon: ZaiLogo, monochrome: false },
};

/** Resolves the mark for a model's lab, falling back to a neutral tool glyph. */
export const ProviderMarkFor = (provider_id: string | undefined): EngineMark =>
	(provider_id === undefined ? undefined : provider_marks[provider_id]) ?? unknown_engine_mark;

const provider_inference_patterns: ReadonlyArray<readonly [RegExp, string]> = [
	[/^claude(?:[-_.]|$)/u, "anthropic"],
	[/^(?:chatgpt|codex|gpt|o[1345])(?:[-_.]|$)/u, "openai"],
	[/^(?:gemini|gemma)(?:[-_.]|$)/u, "google"],
	[/^grok(?:[-_.]|$)/u, "xai"],
	[/^nemotron(?:[-_.]|$)/u, "nvidia"],
	[/^muse(?:[-_.]|$)/u, "meta"],
	[/^hy3(?:[-_.]|$)/u, "tencent"],
	[/^mimo(?:[-_.]|$)/u, "xiaomi"],
	[/^minimax(?:[-_.]|$)/u, "minimax"],
	[/^glm(?:[-_.]|$)/u, "zai"],
	[/^kimi(?:[-_.]|$)/u, "moonshot"],
	[/^qwen(?:[-_.]|$)/u, "qwen"],
	[/^deepseek(?:[-_.]|$)/u, "deepseek"],
];

const unqualified_model_id = (model_id: string) => model_id.split("/").at(-1) ?? model_id;

/** Infers a lab from a raw model id when the catalog has no entry for it. */
export const InferProviderIdForModelId = (model_id: string | undefined): string | undefined => {
	if (model_id === undefined) return undefined;
	const normalized = unqualified_model_id(model_id).toLowerCase();
	const matched = provider_inference_patterns.find(([pattern]) => pattern.test(normalized));
	return matched?.[1];
};

/** Humanizes a raw id as a last resort when no catalog entry matches. */
export const HumanizeModelId = (model_id: string): string => {
	const base = unqualified_model_id(model_id);
	// Keep version numbers intact, replace separators with spaces and title-case words.
	return base
		.split(/[-_]+/u)
		.map((segment) => (segment.length === 0 ? segment : segment.charAt(0).toUpperCase() + segment.slice(1)))
		.join(" ");
};

/** Names the Tailwind classes that size a provider mark and keep it theme-correct. */
export const EngineMarkClass = (mark: EngineMark, size = "size-5") => {
	const color = mark.muted === true ? " text-muted-foreground" : "";
	return mark.monochrome ? `${size} shrink-0 dark:invert${color}` : `${size} shrink-0${color}`;
};

export type CatalogLike = {
	readonly manifest: {
		readonly models: ReadonlyArray<{
			readonly harness: string;
			readonly id: string;
			readonly name: string;
			readonly native_model_id: string;
			readonly native_selection?: { readonly model_id: string };
			readonly provider: string;
		}>;
	};
};

/** Presents one usage slice: the model's catalog name and its lab's mark. */
export interface UsageSlicePresentation {
	readonly label: string;
	readonly mark: EngineMark;
}

const find_model_in_catalog = (
	catalog: CatalogLike | undefined,
	engine_id: string | undefined,
	model_id: string,
): CatalogLike["manifest"]["models"][number] | undefined => {
	if (catalog === undefined) return undefined;
	const unqualified = unqualified_model_id(model_id);
	const alias = model_id.endsWith("-pro") ? model_id.slice(0, -4) : undefined;
	const alias_unqualified = alias !== undefined ? unqualified_model_id(alias) : undefined;
	return catalog.manifest.models.find((candidate) => {
		if (engine_id !== undefined && candidate.harness !== engine_id) return false;
		return (
			candidate.native_model_id === model_id ||
			candidate.native_selection?.model_id === model_id ||
			candidate.native_model_id === unqualified ||
			(candidate.native_selection?.model_id !== undefined &&
				candidate.native_selection.model_id === unqualified) ||
			(alias !== undefined && candidate.native_model_id === alias) ||
			(alias_unqualified !== undefined && candidate.native_model_id === alias_unqualified)
		);
	});
};

export const PresentationForModelInCatalog = (
	engine_id: string | undefined,
	model_id: string | undefined,
	catalog: CatalogLike | undefined,
): UsageSlicePresentation => {
	if (model_id === undefined) {
		return { label: EngineDisplayName(engine_id), mark: EngineMarkFor(engine_id) };
	}
	const model = find_model_in_catalog(catalog, engine_id, model_id);
	if (model !== undefined) return { label: model.name, mark: ProviderMarkFor(model.provider) };
	const fallback_model = find_model_in_catalog(catalog, undefined, model_id);
	if (fallback_model !== undefined)
		return { label: fallback_model.name, mark: ProviderMarkFor(fallback_model.provider) };
	const inferred = InferProviderIdForModelId(model_id);
	if (inferred !== undefined) return { label: HumanizeModelId(model_id), mark: ProviderMarkFor(inferred) };
	return { label: HumanizeModelId(model_id), mark: EngineMarkFor(engine_id) };
};

/**
 * Resolves how a usage slice reads and which mark it wears. A catalog model
 * shows its name under its lab's mark (a Cursor-hosted GPT model wears the
 * OpenAI mark); a model the catalog no longer lists shows its raw id; a slice
 * without a model falls back to the engine's own name and mark.
 */
export const UsageSlicePresentationFor = (
	engine_id: string | undefined,
	model_id: string | undefined,
): UsageSlicePresentation =>
	PresentationForModelInCatalog(engine_id, model_id, { manifest: model_manifest });
