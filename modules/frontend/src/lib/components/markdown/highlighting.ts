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

/** An opened fenced block is the reason to fetch Shiki and its grammars. */
const fence_info = /(?:^|\n) {0,3}(?:`{3,}|~{3,})[ \t]*([^\s`~{]+)/gu;

const fence_opening = /^ {0,3}(`{3,}|~{3,})[ \t]*(.*)$/u;

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

export type ConversationFence = {
	/** The fence's own info label, lowercased — `mermaid` and friends included. */
	readonly label: string | undefined;
	/** The grammar this fence asked for, when the label names one we carry. */
	readonly language: string | undefined;
	/** Body text as Markdown-it reports it: every line, each newline-terminated. */
	readonly body: string;
	/** A fence the source has terminated can never change again. */
	readonly closed: boolean;
};

/** A closing run must repeat the opener's character at least as many times. */
const is_conversation_fence_closing = (line: string, marker: string): boolean => {
	const closing = /^ {0,3}(`+|~+)[ \t]*$/u.exec(line);
	const run = closing?.[1];
	if (run === undefined) return false;
	return run[0] === marker[0] && run.length >= marker.length;
};

/**
 * Walks fences in source order so a caller can tell a finished block from the
 * one still being written. Streaming appends, so the unterminated fence is the
 * message's live tail — everything before it is final and safe to render.
 */
export const ScanConversationFences = (markdown: string): ReadonlyArray<ConversationFence> => {
	const lines = markdown.split("\n");
	const fences: Array<ConversationFence> = [];
	let index = 0;

	while (index < lines.length) {
		const opening = fence_opening.exec(lines[index] ?? "");
		const marker = opening?.[1];
		const info = opening?.[2] ?? "";
		index += 1;
		/** Commonmark forbids a backtick in a backtick fence's info string. */
		if (marker === undefined || (marker.startsWith("`") && info.includes("`"))) continue;

		const body: Array<string> = [];
		let closed = false;
		while (index < lines.length) {
			const line = lines[index] ?? "";
			index += 1;
			if (is_conversation_fence_closing(line, marker)) {
				closed = true;
				break;
			}
			body.push(line);
		}

		const label = info.split("[", 1).at(0)?.split("{", 1).at(0)?.trim().toLowerCase();
		fences.push({
			body: body.length === 0 ? "" : `${body.join("\n")}\n`,
			closed,
			label: label === undefined || label.length === 0 ? undefined : label,
			language: label === undefined ? undefined : conversation_fence_languages[label],
		});
	}

	return fences;
};

/** Trailing blank lines are Markdown-it bookkeeping, not part of the block. */
const normalize_conversation_fence_body = (body: string): string => body.replace(/\n+$/u, "");

/**
 * The body of the fence the message has not yet terminated. Rendering it would
 * re-tokenise a block that grows with every revealed word, so the renderer
 * leaves exactly this one plain until its closing run arrives.
 */
export const OpenConversationFenceBody = (markdown: string): string | undefined => {
	const open = ScanConversationFences(markdown).find((fence) => !fence.closed);
	return open === undefined ? undefined : normalize_conversation_fence_body(open.body);
};

export const is_open_conversation_fence_body = (
	code: string,
	open_body: string | undefined,
): boolean => open_body !== undefined && normalize_conversation_fence_body(code) === open_body;

/**
 * The highlighter chunk once it has evaluated. A resident module lets settled
 * messages whose grammars are already registered build their full plugin set
 * synchronously and parse exactly once at mount.
 */
let resident_settled_highlighting: typeof import("./settled-highlighting") | undefined;

const LoadSettledHighlighting = Effect.gen(function* () {
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
	if (module !== undefined) resident_settled_highlighting = module;
	return module;
});

/**
 * Mid-stream fence highlighting. Grammars are requested from an *opened*
 * fence rather than a closed one, so the highlighter is resident by the time
 * the first block closes; each closed fence is then tokenised exactly once
 * while the fence the message has not terminated stays plain.
 *
 * Highlighting is an optional renderer enhancement. If the chunk or a grammar
 * cannot load, the message still renders with unhighlighted code.
 */
export const LoadConversationStreamingHighlightPlugin = (
	markdown: string,
	get_open_fence_body: () => string | undefined,
) =>
	Effect.gen(function* () {
		const languages = RequestedConversationFenceLanguages(markdown);
		if (languages.length === 0) return undefined;
		const module = yield* LoadSettledHighlighting;
		if (module === undefined) return undefined;
		yield* module.RegisterConversationLanguages(languages).pipe(Effect.ignore);
		return module.MakeConversationFenceHighlightPlugin(get_open_fence_body);
	});

/**
 * Settled code highlighting is an optional renderer enhancement. Native module
 * caching ensures concurrent messages share the one Shiki payload. If that
 * optional chunk cannot load, the message still renders with unhighlighted code.
 */
export const LoadConversationSettledMarkdownPlugins = (markdown: string) =>
	Effect.gen(function* () {
		const languages = RequestedConversationFenceLanguages(markdown);
		if (languages.length === 0) return conversation_rich_markdown_plugins;
		const module = yield* LoadSettledHighlighting;
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

/**
 * The settled plugin set, synchronously, when every requested grammar is
 * already registered — the common case for reopened threads once one message
 * has highlighted. Returns undefined when a grammar still needs loading.
 */
export const TryResidentConversationSettledMarkdownPlugins = (
	markdown: string,
): ReadonlyArray<ComarkPlugin> | undefined => {
	const languages = RequestedConversationFenceLanguages(markdown);
	if (languages.length === 0) return conversation_rich_markdown_plugins;
	const module = resident_settled_highlighting;
	if (module === undefined || !module.AreConversationLanguagesResident(languages))
		return undefined;
	return [...conversation_rich_markdown_plugins, module.MakeConversationFenceHighlightPlugin()];
};

/**
 * The fence plugin stays synchronous inside the parse pipeline, so a fence
 * whose tokens are not cached yet renders plain and registers itself as
 * pending. The rendering workers drain that set between parses.
 */
export const HasPendingConversationFenceTokenization = (): boolean =>
	resident_settled_highlighting?.HasPendingConversationFenceTokenization() ?? false;

export const TokenizePendingConversationFences = Effect.gen(function* () {
	const module = resident_settled_highlighting;
	if (module === undefined) return;
	yield* module.TokenizePendingConversationFences;
});

/** Comark's token processor emits fence bodies with their final newline removed. */
const parsed_fence_body = (body: string): string =>
	body.endsWith("\n") ? body.slice(0, -1) : body;

/** Pre-tokenizes every terminated known-language fence before a settled parse. */
export const WarmConversationFenceTokens = (markdown: string) =>
	Effect.gen(function* () {
		const module = resident_settled_highlighting;
		if (module === undefined) return;
		const requests = ScanConversationFences(markdown)
			.filter((fence) => fence.closed && fence.language !== undefined)
			.map((fence) => ({ body: parsed_fence_body(fence.body), language: fence.language }));
		if (requests.length === 0) return;
		yield* module.TokenizeConversationFences(requests);
	});
