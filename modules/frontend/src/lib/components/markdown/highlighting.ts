import math from "comark/plugins/math";
import mermaid from "comark/plugins/mermaid";
import type { ComarkPlugin } from "comark";
import { Data, Effect } from "effect";

export const conversation_math_plugin = math();
export const conversation_mermaid_plugin = mermaid();

/** Rich block recognition remains cheap enough to keep in every settled message. */
export const conversation_rich_markdown_plugins = [
	conversation_math_plugin,
	conversation_mermaid_plugin,
];

export class ConversationHighlightLoadFailure extends Data.TaggedError(
	"ConversationHighlightLoadFailure",
)<{
	readonly cause: unknown;
}> {}

/** A complete fenced block is the only reason to fetch Shiki and its grammars. */
const fence_info = /(?:^|\n) {0,3}(?:`{3,}|~{3,})[ \t]*([^\s`~{]+)/gu;

const conversation_fence_languages: Readonly<Record<string, string>> = {
	astro: "astro",
	bash: "bash",
	c: "c",
	"c#": "csharp",
	"c++": "cpp",
	cpp: "cpp",
	csharp: "csharp",
	cs: "csharp",
	css: "css",
	cxx: "cpp",
	go: "go",
	golang: "go",
	htm: "html",
	html: "html",
	java: "java",
	javascript: "javascript",
	js: "javascript",
	jsx: "jsx",
	markdown: "markdown",
	md: "markdown",
	mjs: "javascript",
	ps: "powershell",
	ps1: "powershell",
	powershell: "powershell",
	py: "python",
	python: "python",
	rs: "rust",
	rust: "rust",
	sh: "bash",
	shell: "bash",
	sql: "sql",
	svelte: "svelte",
	toml: "toml",
	ts: "typescript",
	tsx: "tsx",
	typescript: "typescript",
	vue: "vue",
	xhtml: "xml",
	xml: "xml",
	yaml: "yaml",
	yml: "yaml",
	json: "json",
};

/** Collects only known fence grammars; unknown labels leave code unhighlighted. */
export const RequestedConversationFenceLanguages = (markdown: string): ReadonlyArray<string> => {
	const requested = new Set<string>();
	for (const match of markdown.matchAll(fence_info)) {
		const without_filename = match[1]?.toLowerCase().split("[", 1).at(0);
		const label = without_filename?.split("{", 1).at(0);
		const language = label === undefined ? undefined : conversation_fence_languages[label];
		if (language !== undefined) requested.add(language);
	}
	return [...requested];
};

/**
 * Settled code highlighting is an optional renderer enhancement. Native module
 * caching ensures concurrent messages share the one Shiki payload. If that
 * optional chunk cannot load, the message still renders with unhighlighted code.
 */
export const LoadConversationSettledMarkdownPlugins = (markdown: string) =>
	Effect.gen(function* () {
		const languages = RequestedConversationFenceLanguages(markdown);
		if (languages.length === 0) return conversation_rich_markdown_plugins;
		const module = yield* Effect.tryPromise({
			catch: (cause) => new ConversationHighlightLoadFailure({ cause }),
			try: () => import("./settled-highlighting"),
		}).pipe(
			Effect.catchTag("ConversationHighlightLoadFailure", () =>
				Effect.gen(function* () {
					return undefined;
				}),
			),
		);
		return module === undefined
			? conversation_rich_markdown_plugins
			: yield* module.LoadConversationSettledMarkdownPlugins(languages).pipe(
					Effect.catch(() =>
						Effect.gen(function* () {
							return conversation_rich_markdown_plugins;
						}),
					),
				);
	});

/** Streaming carries no rich parsing or synchronous highlighter work. */
export const create_conversation_streaming_markdown_plugins = (
	streaming_words_plugin: ComarkPlugin,
) => [streaming_words_plugin];
