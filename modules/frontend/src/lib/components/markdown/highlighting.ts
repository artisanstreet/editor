import highlight from "comark/plugins/highlight";
import math from "comark/plugins/math";
import mermaid from "comark/plugins/mermaid";
import c from "shiki/dist/langs/c.mjs";
import cpp from "shiki/dist/langs/cpp.mjs";
import csharp from "shiki/dist/langs/csharp.mjs";
import css from "shiki/dist/langs/css.mjs";
import go from "shiki/dist/langs/go.mjs";
import html from "shiki/dist/langs/html.mjs";
import java from "shiki/dist/langs/java.mjs";
import jsx from "shiki/dist/langs/jsx.mjs";
import markdown from "shiki/dist/langs/markdown.mjs";
import powershell from "shiki/dist/langs/powershell.mjs";
import python from "shiki/dist/langs/python.mjs";
import rust from "shiki/dist/langs/rust.mjs";
import sql from "shiki/dist/langs/sql.mjs";
import toml from "shiki/dist/langs/toml.mjs";
import xml from "shiki/dist/langs/xml.mjs";
import github_dark from "shiki/dist/themes/github-dark.mjs";
import github_light from "shiki/dist/themes/github-light.mjs";
import type { ComarkPlugin } from "comark";

/**
 * Barekey's dual GitHub theme, adapted to Comark's renderer-safe AST plugin.
 * Common agent languages missing from Comark's default bundle are preloaded;
 * an unknown language still falls back to an unhighlighted code block.
 */
export const conversation_highlight_plugin = highlight({
	languages: [
		c,
		cpp,
		csharp,
		css,
		go,
		html,
		java,
		jsx,
		markdown,
		powershell,
		python,
		rust,
		sql,
		toml,
		xml,
	],
	registerDefaultThemes: false,
	themes: { dark: github_dark, light: github_light },
});

export const conversation_math_plugin = math();
export const conversation_mermaid_plugin = mermaid();

export const create_conversation_streaming_markdown_plugins = (
	streaming_words_plugin: ComarkPlugin,
) => [conversation_highlight_plugin, streaming_words_plugin];
export const conversation_markdown_plugins = [
	conversation_math_plugin,
	conversation_mermaid_plugin,
	conversation_highlight_plugin,
];
