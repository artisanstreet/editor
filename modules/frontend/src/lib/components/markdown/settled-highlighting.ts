import { getHighlighter, highlightCodeBlocks } from "comark/plugins/highlight";
import github_dark from "shiki/dist/themes/github-dark.mjs";
import github_light from "shiki/dist/themes/github-light.mjs";
import type { LanguageRegistration } from "shiki";
import type { ComarkElement, ComarkNode, ComarkPlugin } from "comark";
import { Effect } from "effect";

import {
	conversation_rich_markdown_plugins,
	ConversationHighlightLoadFailure,
	is_open_conversation_fence_body,
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

/**
 * Barekey's dual GitHub theme. Grammars are supplied only after a message
 * proves which fenced languages it actually uses.
 */
const MakeConversationHighlightOptions = (
	languages: ReadonlyArray<ConversationLanguageRegistration>,
) => ({
	languages: [...languages],
	registerDefaultLanguages: false,
	registerDefaultThemes: false,
	themes: { dark: github_dark, light: github_light },
});

/**
 * Grammars whose registration with the shared highlighter has completed.
 * Resident languages let a settled message build its full plugin set
 * synchronously and parse exactly once at mount.
 */
const resident_languages = new Set<string>();

export const AreConversationLanguagesResident = (languages: ReadonlyArray<string>): boolean =>
	languages.every((language) => resident_languages.has(language));

/**
 * Loads and registers the requested grammars with the shared highlighter.
 * Comark shares one highlighter promise. A concurrent request made during its
 * first initialization inherits the first request's languages, so run
 * registration once more after that promise settles. The second pass uses
 * Comark's loaded-language set and only installs any grammar still missing.
 */
export const RegisterConversationLanguages = (languages: ReadonlyArray<string>) =>
	Effect.gen(function* () {
		const loaded = yield* Effect.forEach(languages, (language) =>
			LoadLanguage(language).pipe(Effect.option),
		);
		const language_registrations: Array<ConversationLanguageRegistration> = [];
		const loadable: Array<string> = [];
		for (const [index, language] of loaded.entries()) {
			const name = languages[index];
			if (language._tag === "Some" && language.value !== undefined && name !== undefined) {
				language_registrations.push(language.value);
				loadable.push(name);
			}
		}
		const options = MakeConversationHighlightOptions(language_registrations);
		yield* Effect.tryPromise({
			catch: (cause) => new ConversationHighlightLoadFailure({ cause }),
			try: () => getHighlighter(options),
		});
		yield* Effect.tryPromise({
			catch: (cause) => new ConversationHighlightLoadFailure({ cause }),
			try: () => getHighlighter(options),
		});
		for (const name of loadable) resident_languages.add(name);
	});

type ConversationFenceTokens = {
	readonly class_name: string;
	readonly children: ReadonlyArray<ComarkNode>;
};

export type ConversationFenceRequest = {
	readonly body: string;
	readonly highlights?: unknown;
	readonly language?: unknown;
};

/**
 * Tokenized fence bodies shared across parses and messages. A fence is
 * tokenised exactly once — normally right after it closes mid-stream — and
 * every later parse of the same body (the settle reparse above all) reuses
 * the tokens by reference.
 */
const fence_token_cache = new Map<string, ConversationFenceTokens>();
const fence_token_cache_limit = 256;
/**
 * Fences a synchronous parse saw before their tokens existed. The rendering
 * workers drain this between parses; the plugin itself must stay synchronous
 * inside the parse pipeline.
 */
const pending_fence_requests = new Map<string, ConversationFenceRequest>();

/** Attribute shape varies between parse paths; the key normalizes it away. */
const fence_cache_key = (request: ConversationFenceRequest): string => {
	const language = typeof request.language === "string" ? request.language : null;
	const highlights =
		Array.isArray(request.highlights) && request.highlights.length > 0
			? request.highlights
			: null;
	return JSON.stringify([language, highlights, request.body]);
};

const store_fence_tokens = (key: string, entry: ConversationFenceTokens) => {
	fence_token_cache.set(key, entry);
	if (fence_token_cache.size > fence_token_cache_limit) {
		const oldest = fence_token_cache.keys().next().value;
		if (oldest !== undefined) fence_token_cache.delete(oldest);
	}
};

const conversation_fence_options = MakeConversationHighlightOptions([]);

const TokenizeFenceRequest = (key: string, request: ConversationFenceRequest) =>
	Effect.gen(function* () {
		const highlighted = yield* Effect.tryPromise({
			catch: (cause) => new ConversationHighlightLoadFailure({ cause }),
			try: () =>
				highlightCodeBlocks(
					{
						frontmatter: {},
						meta: {},
						nodes: [
							[
								"pre",
								{ highlights: request.highlights, language: request.language },
								["code", {}, request.body],
							],
						],
					},
					conversation_fence_options,
				),
		}).pipe(
			Effect.catchTag("ConversationHighlightLoadFailure", () =>
				Effect.gen(function* () {
					return undefined;
				}),
			),
		);
		const produced = highlighted?.nodes[0];
		/**
		 * A highlighter-level failure is transient — do not cache it, or the
		 * body would stay plain for every later parse including the settle.
		 * Per-block failures (unknown grammar) are deterministic: the library
		 * embeds them as a plain shiki block, and that result caches normally.
		 */
		if (!Array.isArray(produced) || !Array.isArray(produced[2])) return;
		const produced_attributes = (produced[1] ?? {}) as { class?: string };
		store_fence_tokens(key, {
			children: (produced[2] as ComarkElement).slice(2) as ReadonlyArray<ComarkNode>,
			class_name: produced_attributes.class ?? "shiki",
		});
	});

export const HasPendingConversationFenceTokenization = (): boolean =>
	pending_fence_requests.size > 0;

/** Tokenizes every fence a synchronous parse could not substitute yet. */
export const TokenizePendingConversationFences = Effect.gen(function* () {
	while (pending_fence_requests.size > 0) {
		const next = pending_fence_requests.entries().next();
		if (next.done === true) break;
		const [key, request] = next.value;
		pending_fence_requests.delete(key);
		if (fence_token_cache.has(key)) continue;
		yield* TokenizeFenceRequest(key, request);
	}
});

/** Pre-tokenizes known fence bodies so the following parse substitutes them all. */
export const TokenizeConversationFences = (requests: ReadonlyArray<ConversationFenceRequest>) =>
	Effect.gen(function* () {
		for (const request of requests) {
			const key = fence_cache_key(request);
			if (fence_token_cache.has(key)) continue;
			yield* TokenizeFenceRequest(key, request);
		}
	});

const is_plain_fence = (node: ComarkElement): boolean =>
	node[0] === "pre" &&
	Array.isArray(node[2]) &&
	node[2][0] === "code" &&
	typeof node[2][2] === "string";

/**
 * Substitutes cached tokens into one fence, or records it as pending. The
 * parse pipeline is synchronous by discipline, so a cache miss renders plain
 * this parse; the owning worker tokenizes and re-parses immediately after.
 */
const SubstituteFence = (pre: ComarkElement): ComarkNode | undefined => {
	const attributes = (pre[1] ?? {}) as Record<string, unknown>;
	const code = pre[2] as ComarkElement;
	const body = code[2];
	if (typeof body !== "string") return undefined;
	const request: ConversationFenceRequest = {
		body,
		highlights: attributes.highlights,
		language: attributes.language,
	};
	const key = fence_cache_key(request);
	const entry = fence_token_cache.get(key);
	if (entry === undefined) {
		pending_fence_requests.set(key, request);
		return undefined;
	}
	return [
		"pre",
		{ ...attributes, class: entry.class_name },
		["code", (code[1] ?? {}) as Record<string, unknown>, ...entry.children],
	];
};

/** Copy-on-write walk: untouched siblings and branches keep their references. */
const HighlightFenceNodes = (
	nodes: ReadonlyArray<ComarkNode>,
	open_body: string | undefined,
): ReadonlyArray<ComarkNode> => {
	let next: Array<ComarkNode> | undefined;
	for (let index = 0; index < nodes.length; index += 1) {
		const node = nodes[index];
		if (node === undefined || typeof node === "string" || !Array.isArray(node)) continue;
		if (typeof node[0] !== "string") continue;
		const element = node as ComarkElement;
		let replacement: ComarkNode | undefined;
		if (is_plain_fence(element)) {
			const body = (element[2] as ComarkElement)[2] as string;
			if (is_open_conversation_fence_body(body, open_body)) continue;
			replacement = SubstituteFence(element);
		} else {
			const children = element.slice(2) as ReadonlyArray<ComarkNode>;
			const rewritten = HighlightFenceNodes(children, open_body);
			replacement =
				rewritten === children
					? undefined
					: ([element[0], element[1], ...rewritten] as ComarkElement);
		}
		if (replacement !== undefined) {
			next ??= [...nodes];
			next[index] = replacement;
		}
	}
	return next ?? nodes;
};

/**
 * Highlights terminated fences from the shared token cache.
 *
 * During streaming the plugin sees only the incrementally parsed tail (nodes
 * before `reusableNodes` are its own earlier output), skips the one fence the
 * message has not terminated — re-tokenising a block that grows with every
 * revealed word is the cost this design retires — and mutates `state.tree`
 * rather than replacing it, because the parser's reuse chain holds the same
 * object.
 */
export const MakeConversationFenceHighlightPlugin = (
	get_open_fence_body?: () => string | undefined,
): ComarkPlugin => ({
	name: "conversation-fence-highlight",
	post: (state) => {
		const reused: number = Array.isArray(state.reusableNodes) ? state.reusableNodes.length : 0;
		const open_body = get_open_fence_body?.();
		const stable = state.tree.nodes.slice(0, reused);
		const fresh = HighlightFenceNodes(state.tree.nodes.slice(reused), open_body);
		state.tree.nodes = [...stable, ...fresh];
	},
});

/** The full settled plugin set, grammar registration included. */
export const LoadConversationSettledMarkdownPlugins = (languages: ReadonlyArray<string>) =>
	RegisterConversationLanguages(languages).pipe(
		Effect.map(() => [
			...conversation_rich_markdown_plugins,
			MakeConversationFenceHighlightPlugin(),
		]),
	);
