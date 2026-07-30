import type { LanguageSupport } from "@codemirror/language";
import { Data, Effect, Option } from "effect";

/**
 * Lezer grammar selection.
 *
 * Lezer is the default rather than a colour-only highlighter because CodeMirror
 * reads the same tree for auto-indent, folding, bracket matching, and
 * structural selection. Choosing a highlighter here therefore chooses those
 * behaviours too, which is why a token-stream highlighter is not a drop-in
 * alternative for the languages listed below.
 *
 * Every grammar is loaded on demand: the editor chunk carries none of them, and
 * opening a `.rs` file never downloads the Python parser.
 */

/** The identifier the rest of the app uses for a language, independent of grammar package. */
export type EditorLanguageId =
	| "css"
	| "go"
	| "html"
	| "javascript"
	| "json"
	| "markdown"
	| "plaintext"
	| "python"
	| "rust"
	| "sql"
	| "typescript"
	| "xml"
	| "yaml";

export class EditorLanguageLoadError extends Data.TaggedError("EditorLanguageLoadError")<{
	readonly cause: unknown;
	readonly language: EditorLanguageId;
}> {}

const Import = <A>(load: () => Promise<A>) => Effect.tryPromise(load);
const grammars: Readonly<
	Record<Exclude<EditorLanguageId, "plaintext">, Effect.Effect<LanguageSupport, unknown>>
> = {
	css: Import(() => import("@codemirror/lang-css")).pipe(Effect.map((module) => module.css())),
	go: Import(() => import("@codemirror/lang-go")).pipe(Effect.map((module) => module.go())),
	html: Import(() => import("@codemirror/lang-html")).pipe(Effect.map((module) => module.html())),
	javascript: Import(() => import("@codemirror/lang-javascript")).pipe(
		Effect.map((module) => module.javascript({ jsx: true })),
	),
	json: Import(() => import("@codemirror/lang-json")).pipe(Effect.map((module) => module.json())),
	markdown: Import(() => import("@codemirror/lang-markdown")).pipe(
		Effect.map((module) => module.markdown()),
	),
	python: Import(() => import("@codemirror/lang-python")).pipe(
		Effect.map((module) => module.python()),
	),
	rust: Import(() => import("@codemirror/lang-rust")).pipe(Effect.map((module) => module.rust())),
	sql: Import(() => import("@codemirror/lang-sql")).pipe(Effect.map((module) => module.sql())),
	typescript: Import(() => import("@codemirror/lang-javascript")).pipe(
		Effect.map((module) => module.javascript({ jsx: true, typescript: true })),
	),
	xml: Import(() => import("@codemirror/lang-xml")).pipe(Effect.map((module) => module.xml())),
	yaml: Import(() => import("@codemirror/lang-yaml")).pipe(Effect.map((module) => module.yaml())),
};

const by_extension = new Map<string, EditorLanguageId>([
	["cjs", "javascript"],
	["css", "css"],
	["go", "go"],
	["htm", "html"],
	["html", "html"],
	["js", "javascript"],
	["json", "json"],
	["jsonc", "json"],
	["jsx", "javascript"],
	["md", "markdown"],
	["mdx", "markdown"],
	["mjs", "javascript"],
	["mts", "typescript"],
	["py", "python"],
	["pyi", "python"],
	["rs", "rust"],
	["scss", "css"],
	["sql", "sql"],
	/**
	 * Svelte is HTML-shaped at the top level, so the HTML grammar highlights
	 * markup and inline blocks correctly while script contents stay plain. A
	 * dedicated Svelte grammar would be strictly better and is not yet wired.
	 */
	["sv", "html"],
	["svelte", "html"],
	["ts", "typescript"],
	["tsx", "typescript"],
	["xml", "xml"],
	["yaml", "yaml"],
	["yml", "yaml"],
]);

/**
 * Some files are recognised by name rather than extension, so a `Dockerfile` or
 * a `.gitignore` still lands on a sensible grammar.
 */
const by_filename = new Map<string, EditorLanguageId>([
	[".babelrc", "json"],
	[".prettierrc", "json"],
	["dockerfile", "plaintext"],
	["makefile", "plaintext"],
]);

/**
 * Resolves the grammar for a path, preferring the backend's declared language
 * when it names one this editor knows. The declared value wins because the
 * workspace has read the file; the extension is the fallback, not the truth.
 */
export const EditorLanguageForPath = (
	path: string,
	declared_language?: string,
): EditorLanguageId => {
	if (declared_language !== undefined && declared_language in grammars)
		return declared_language as EditorLanguageId;

	const filename = path.split("/").at(-1)?.toLowerCase() ?? "";
	const named = by_filename.get(filename);
	if (named !== undefined) return named;

	/** A leading dot means the whole name is the extension (`.gitignore`). */
	const extension = filename.startsWith(".")
		? filename.slice(1)
		: (filename.split(".").at(-1) ?? "");

	return by_extension.get(extension) ?? "plaintext";
};

/** True when the language has a grammar to load, so callers can skip the round trip. */
export const EditorLanguageIsHighlighted = (language: EditorLanguageId) => language !== "plaintext";

/**
 * Loads one grammar. Resolves to `undefined` rather than failing when the
 * language has no grammar or its chunk cannot be fetched — an unhighlighted
 * file is a degraded editor, not a broken one.
 */
export const LoadEditorLanguage = (
	language: EditorLanguageId,
): Effect.Effect<Option.Option<LanguageSupport>, EditorLanguageLoadError> =>
	language === "plaintext"
		? Effect.succeed(Option.none())
		: grammars[language].pipe(
				Effect.mapError((cause) => new EditorLanguageLoadError({ cause, language })),
				Effect.map(Option.some),
			);
