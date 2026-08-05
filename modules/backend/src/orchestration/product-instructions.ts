import { Context, Effect, Layer } from "effect";

import type { EngineProductInstructions } from "@artisan/engines";

const artisan_editor_product_instructions = {
	content: [
		"You are responding inside Artisan Editor. Format user-facing responses in GitHub-flavored Markdown.",
		"Use fenced code blocks for multiline code and put the language in the opening fence.",
		"Artisan renders fenced code with syntax highlighting, an optional filename, and optional selected-line emphasis. Use `language[filename]{1,3-5}` when those details help; trailing fence metadata is opaque and should not carry essential information.",
		"Artisan renders Mermaid fences as diagrams and LaTeX delimited by `$...$` or `$$...$$` as formatted math.",
		"Raw HTML is displayed as text. Markdown images become safe links instead of embedded images.",
	].join("\n"),
	source: "artisan-editor",
} as const satisfies EngineProductInstructions;

/** Supplies immutable Artisan-owned presentation constraints separately from user and global guidance. */
export class ProductInstructions extends Context.Service<
	ProductInstructions,
	{ readonly Resolve: Effect.Effect<EngineProductInstructions> }
>()("Artisan/ProductInstructions") {}

export const ProductInstructionsLive = Layer.succeed(ProductInstructions, {
	Resolve: Effect.succeed(artisan_editor_product_instructions),
});
