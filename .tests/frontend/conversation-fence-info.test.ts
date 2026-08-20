import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

import { parse_conversation_markdown } from "../../modules/frontend/src/lib/components/markdown/test-parsing";

import { ReadStylesheets } from "./stylesheet-source";

const ReadSource = (path: string) =>
	readFileSync(resolve(import.meta.dirname, "../..", path), "utf8");

describe("conversation code-fence info strings", () => {
	it("parses the language, filename, selected lines, and opaque trailing metadata", async () => {
		const tree = await parse_conversation_markdown(
			[
				"```typescript[src/lib/example.ts]{2,4-5} title=Demo",
				"const answer: number = 42;",
				"```",
			].join("\n"),
		);

		const code_block = tree.nodes[0];
		expect(code_block).toEqual(
			expect.arrayContaining([
				"pre",
				expect.objectContaining({
					class: expect.stringContaining("shiki"),
					filename: "src/lib/example.ts",
					highlights: [2, 4, 5],
					language: "typescript",
					meta: "title=Demo",
				}),
			]),
		);
		expect(JSON.stringify(code_block)).toContain("--shiki-dark");
	});

	it("maps highlighted pre nodes into the Barekey code-snippet presentation", () => {
		const content = ReadSource("modules/frontend/src/lib/components/markdown/content.svelte");
		const styles = ReadStylesheets();

		expect(content).toContain("ProsePre: CodeSnippet");
		expect(content).toContain("tree={rendered_tree}");
		expect(styles).toContain(".docs-code-snippet-body .line.highlight");
		expect(styles).toContain("display: inline-block !important");
		expect(styles).toContain("color: var(--shiki-dark) !important");
	});

	it("marks selected lines in the highlighted AST", async () => {
		const tree = await parse_conversation_markdown(
			["```ts{2}", "const first = 1;", "const selected = 2;", "```"].join("\n"),
		);
		const rendered = JSON.stringify(tree.nodes[0]);

		expect(rendered).toContain('"class":"line highlight"');
		expect(rendered).toContain('" selected"');
	});

	it("falls back to plain code when a language grammar is unknown", async () => {
		const tree = await parse_conversation_markdown("```artisan-unknown\nvalue\n```");

		expect(tree.nodes[0]).toEqual(
			expect.arrayContaining([
				"pre",
				expect.objectContaining({
					class: expect.stringContaining("shiki"),
					language: "artisan-unknown",
				}),
			]),
		);
	});
});
