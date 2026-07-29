import {
	SvglClaudeAILogo,
	SvglCursorLogo,
	SvglGrokLogo,
	SvglOpenAILogo,
} from "@selemondev/svgl-svelte";
import Tool from "@tabler/icons-svelte/icons/tool";
import type { Component } from "svelte";

/** Presents one engine's provider mark. @since 0.7.0 */
export interface EngineMark {
	/** The provider's own accent, as a CSS color, for meters and other tinted chrome. */
	readonly accent: string;
	readonly icon: Component;
	/** Marks a single-color logo that must invert with the theme. */
	readonly monochrome: boolean;
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
	icon: Tool,
	monochrome: true,
};

/** Resolves the provider mark for an engine, falling back to a neutral tool glyph. */
export const EngineMarkFor = (engine_id: string | undefined): EngineMark =>
	(engine_id === undefined ? undefined : engine_marks[engine_id]) ?? unknown_engine_mark;

/** Names the Tailwind classes that size a provider mark and keep it theme-correct. */
export const EngineMarkClass = (mark: EngineMark, size = "size-5") =>
	mark.monochrome ? `${size} shrink-0 dark:invert` : `${size} shrink-0`;
