import { HighlightStyle } from "@codemirror/language";
import { EditorView } from "@codemirror/view";
import { tags } from "@lezer/highlight";

/**
 * The editor's appearance, expressed against the shell's own design tokens.
 *
 * CodeMirror themes are ordinary scoped style rules, so every colour here reads
 * a `--color-*`/`--surface-*` variable the rest of Artisan already defines
 * rather than hard-coding a palette. Light and dark therefore follow the app's
 * mode automatically, with no second theme to keep in sync.
 */

export const artisan_theme = EditorView.theme({
	"&": {
		backgroundColor: "transparent",
		color: "var(--foreground)",
		fontSize: "0.8125rem",
		height: "100%",
	},
	".cm-scroller": {
		fontFamily: "var(--font-mono, ui-monospace, monospace)",
		lineHeight: "1.6",
	},
	".cm-content": { caretColor: "var(--foreground)" },
	"&.cm-focused": { outline: "none" },
	".cm-gutters": {
		backgroundColor: "transparent",
		border: "none",
		color: "oklch(from var(--foreground) l c h / 35%)",
	},
	".cm-activeLineGutter": {
		backgroundColor: "transparent",
		color: "oklch(from var(--foreground) l c h / 70%)",
	},
	".cm-activeLine": { backgroundColor: "oklch(from var(--foreground) l c h / 4%)" },
	".cm-selectionBackground, &.cm-focused .cm-selectionBackground, ::selection": {
		backgroundColor: "oklch(from var(--primary) l c h / 25%)",
	},
	".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--foreground)" },
	".cm-matchingBracket, &.cm-focused .cm-matchingBracket": {
		backgroundColor: "oklch(from var(--primary) l c h / 20%)",
		outline: "none",
	},
	".cm-selectionMatch": { backgroundColor: "oklch(from var(--foreground) l c h / 8%)" },
	".cm-foldPlaceholder": {
		backgroundColor: "oklch(from var(--foreground) l c h / 8%)",
		border: "none",
		color: "var(--muted-foreground)",
	},
	".cm-tooltip": {
		backgroundColor: "var(--popover)",
		border: "0.5px solid oklch(from var(--highlight) l c h / 8%)",
		borderRadius: "calc(var(--radius) * 1.4)",
		color: "var(--popover-foreground)",
	},
	".cm-tooltip-autocomplete > ul > li[aria-selected]": {
		backgroundColor: "oklch(from var(--foreground) l c h / 8%)",
		color: "var(--foreground)",
	},
	".cm-panels": {
		backgroundColor: "var(--popover)",
		color: "var(--popover-foreground)",
	},
});

/**
 * Token colours by Lezer tag. Keeping this small and semantic — rather than one
 * rule per grammar node — is what lets a newly added language inherit sensible
 * colours without touching this file.
 */
export const artisan_highlight_style = HighlightStyle.define([
	{ tag: [tags.keyword, tags.moduleKeyword], color: "var(--color-violet-400, #a78bfa)" },
	{
		tag: [tags.controlKeyword, tags.operatorKeyword],
		color: "var(--color-fuchsia-400, #e879f9)",
	},
	{
		tag: [tags.name, tags.deleted, tags.character, tags.propertyName],
		color: "var(--foreground)",
	},
	{
		tag: [tags.function(tags.variableName), tags.function(tags.propertyName), tags.labelName],
		color: "var(--color-blue-400, #60a5fa)",
	},
	{
		tag: [tags.typeName, tags.className, tags.namespace, tags.definition(tags.typeName)],
		color: "var(--color-teal-400, #2dd4bf)",
	},
	{
		tag: [tags.number, tags.bool, tags.null, tags.atom],
		color: "var(--color-amber-400, #fbbf24)",
	},
	{ tag: [tags.string, tags.special(tags.string)], color: "var(--color-emerald-400, #34d399)" },
	{ tag: [tags.regexp, tags.escape], color: "var(--color-orange-400, #fb923c)" },
	{
		tag: [tags.comment, tags.lineComment, tags.blockComment, tags.docComment],
		color: "var(--muted-foreground)",
		fontStyle: "italic",
	},
	{ tag: [tags.meta, tags.processingInstruction], color: "var(--muted-foreground)" },
	{ tag: [tags.tagName, tags.angleBracket], color: "var(--color-rose-400, #fb7185)" },
	{ tag: [tags.attributeName], color: "var(--color-sky-400, #38bdf8)" },
	{ tag: tags.invalid, color: "var(--destructive)" },
	{
		tag: [tags.link, tags.url],
		color: "var(--color-sky-400, #38bdf8)",
		textDecoration: "underline",
	},
	{ tag: tags.heading, color: "var(--foreground)", fontWeight: "600" },
	{ tag: tags.emphasis, fontStyle: "italic" },
	{ tag: tags.strong, fontWeight: "600" },
	{ tag: tags.strikethrough, textDecoration: "line-through" },
]);
