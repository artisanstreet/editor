import { SvglClaudeAILogo, SvglGrokLogo, SvglOpenAILogo } from "@selemondev/svgl-svelte";
import Tool from "@tabler/icons-svelte/icons/tool";
import type { Component } from "svelte";

/** Presents one engine's provider mark. @since 0.7.0 */
export interface EngineMark {
	readonly icon: Component;
	/** Marks a single-color logo that must invert with the theme. */
	readonly monochrome: boolean;
}

const engine_marks: Readonly<Record<string, EngineMark>> = {
	claude: { icon: SvglClaudeAILogo, monochrome: false },
	codex: { icon: SvglOpenAILogo, monochrome: true },
	grok: { icon: SvglGrokLogo, monochrome: true },
};

const unknown_engine_mark: EngineMark = { icon: Tool, monochrome: true };

/** Resolves the provider mark for an engine, falling back to a neutral tool glyph. */
export const EngineMarkFor = (engine_id: string | undefined): EngineMark =>
	(engine_id === undefined ? undefined : engine_marks[engine_id]) ?? unknown_engine_mark;

/** Names the Tailwind classes that size a provider mark and keep it theme-correct. */
export const EngineMarkClass = (mark: EngineMark, size = "size-5") =>
	mark.monochrome ? `${size} shrink-0 dark:invert` : `${size} shrink-0`;
