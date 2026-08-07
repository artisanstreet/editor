import { parse } from "@comark/svelte/parse";
import highlight from "comark/plugins/highlight";
import typescript from "shiki/dist/langs/typescript.mjs";
import github_dark from "shiki/dist/themes/github-dark.mjs";
import github_light from "shiki/dist/themes/github-light.mjs";

import { conversation_rich_markdown_plugins } from "./highlighting";
import { conversation_parse_options } from "./parsing";

const conversation_test_markdown_plugins = [
	...conversation_rich_markdown_plugins,
	highlight({
		languages: [typescript],
		registerDefaultLanguages: false,
		registerDefaultThemes: false,
		themes: { dark: github_dark, light: github_light },
	}),
];

/** Test-only full-dialect parser, deliberately absent from the live renderer graph. */
export const parse_conversation_markdown = (markdown: string) =>
	parse(markdown, {
		...conversation_parse_options,
		plugins: conversation_test_markdown_plugins,
	});
