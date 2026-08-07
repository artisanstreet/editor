import highlight, { getHighlighter } from "comark/plugins/highlight";
import github_dark from "shiki/dist/themes/github-dark.mjs";
import github_light from "shiki/dist/themes/github-light.mjs";
import type { LanguageRegistration } from "shiki";
import { Effect } from "effect";

import {
	conversation_rich_markdown_plugins,
	ConversationHighlightLoadFailure,
} from "./highlighting";

type ConversationLanguageRegistration = LanguageRegistration | Array<LanguageRegistration>;
type LanguageLoader = () => Promise<{ readonly default: ConversationLanguageRegistration }>;

/** Each grammar stays in its own Vite chunk and native module caching shares it across messages. */
const language_loaders: Readonly<Record<string, LanguageLoader>> = {
	astro: () => import("shiki/dist/langs/astro.mjs"),
	bash: () => import("shiki/dist/langs/bash.mjs"),
	c: () => import("shiki/dist/langs/c.mjs"),
	cpp: () => import("shiki/dist/langs/cpp.mjs"),
	csharp: () => import("shiki/dist/langs/csharp.mjs"),
	css: () => import("shiki/dist/langs/css.mjs"),
	go: () => import("shiki/dist/langs/go.mjs"),
	html: () => import("shiki/dist/langs/html.mjs"),
	java: () => import("shiki/dist/langs/java.mjs"),
	javascript: () => import("shiki/dist/langs/javascript.mjs"),
	json: () => import("shiki/dist/langs/json.mjs"),
	jsx: () => import("shiki/dist/langs/jsx.mjs"),
	markdown: () => import("shiki/dist/langs/markdown.mjs"),
	powershell: () => import("shiki/dist/langs/powershell.mjs"),
	python: () => import("shiki/dist/langs/python.mjs"),
	rust: () => import("shiki/dist/langs/rust.mjs"),
	sql: () => import("shiki/dist/langs/sql.mjs"),
	svelte: () => import("shiki/dist/langs/svelte.mjs"),
	toml: () => import("shiki/dist/langs/toml.mjs"),
	tsx: () => import("shiki/dist/langs/tsx.mjs"),
	typescript: () => import("shiki/dist/langs/typescript.mjs"),
	vue: () => import("shiki/dist/langs/vue.mjs"),
	xml: () => import("shiki/dist/langs/xml.mjs"),
	yaml: () => import("shiki/dist/langs/yaml.mjs"),
};

const LoadLanguage = (language: string) =>
	Effect.gen(function* () {
		const loader = language_loaders[language];
		if (loader === undefined) return;
		return yield* Effect.tryPromise({
			catch: (cause) => new ConversationHighlightLoadFailure({ cause }),
			try: loader,
		}).pipe(Effect.map((module) => module.default));
	});

export const LoadConversationSettledMarkdownPlugins = (languages: ReadonlyArray<string>) =>
	Effect.gen(function* () {
		const loaded = yield* Effect.forEach(languages, (language) =>
			LoadLanguage(language).pipe(Effect.option),
		);
		const language_registrations: Array<ConversationLanguageRegistration> = [];
		for (const language of loaded) {
			if (language._tag === "Some" && language.value !== undefined)
				language_registrations.push(language.value);
		}
		const options = MakeConversationHighlightOptions(language_registrations);
		/**
		 * Comark shares one highlighter promise. A concurrent request made during
		 * its first initialization inherits the first request's languages, so run
		 * registration once more after that promise settles. The second pass uses
		 * Comark's loaded-language set and only installs any grammar still missing.
		 */
		yield* Effect.tryPromise({
			catch: (cause) => new ConversationHighlightLoadFailure({ cause }),
			try: () => getHighlighter(options),
		});
		yield* Effect.tryPromise({
			catch: (cause) => new ConversationHighlightLoadFailure({ cause }),
			try: () => getHighlighter(options),
		});
		return [...conversation_rich_markdown_plugins, highlight(options)];
	});

/**
 * Barekey's dual GitHub theme. Grammars are supplied only after a settled
 * message proves which fenced languages it actually uses.
 */
const MakeConversationHighlightOptions = (
	languages: ReadonlyArray<ConversationLanguageRegistration>,
) => ({
	languages: [...languages],
	registerDefaultLanguages: false,
	registerDefaultThemes: false,
	themes: { dark: github_dark, light: github_light },
});
