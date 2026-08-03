import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { createServer, type ViteDevServer } from "vite";
import { afterAll, beforeAll, describe, expect, it } from "vitest";

const workspace = resolve(import.meta.dirname, "../..");
const frontend_root = resolve(workspace, "modules/frontend");
const original_working_directory = process.cwd();
const source = readFileSync(
	resolve(workspace, "modules/frontend/src/lib/components/markdown/code-snippet.sv"),
	"utf8",
);
const strip_ssr_markers = (html: string) => html.replaceAll(/<!--.*?-->/gu, "");

let frontend_vite: ViteDevServer;

const RenderCodeSnippet = async (props: Record<string, unknown>) => {
	const [{ default: CodeSnippet }, svelte, server] = await Promise.all([
		frontend_vite.ssrLoadModule("/src/lib/components/markdown/code-snippet.sv"),
		frontend_vite.ssrLoadModule("svelte"),
		frontend_vite.ssrLoadModule("svelte/server"),
	]);
	const children = svelte.createRawSnippet(() => ({
		render: () =>
			'<code class="shiki language-typescript"><span>const answer = 42;</span></code>',
	}));

	return server.render(CodeSnippet, { props: { ...props, children } }).body;
};

describe("conversation code snippet", () => {
	beforeAll(async () => {
		process.chdir(frontend_root);
		frontend_vite = await createServer({
			appType: "custom",
			configFile: "vite.config.ts",
			server: { middlewareMode: true },
		});
	});

	afterAll(async () => {
		await frontend_vite.close();
		process.chdir(original_working_directory);
	});

	it("server-renders the Barekey filename header around the supplied highlighted code", async () => {
		const body = await RenderCodeSnippet({
			class: "shiki",
			filename: "src/lib/answer.ts",
			highlights: [1],
			language: "typescript",
			meta: "title=Answer",
			style: "position:fixed;inset:0",
		});

		expect(body).toContain('class="docs-code-snippet not-prose"');
		expect(body).toContain('class="docs-code-snippet-header"');
		expect(body).toContain('class="docs-code-snippet-filename"');
		expect(body).toContain("src/lib/answer.ts");
		expect(body).toMatch(/<img src="data:image\/svg\+xml,/u);
		expect(body).toContain('class="docs-code-snippet-body"');
		expect(body).toContain('<pre class="shiki"');
		expect(strip_ssr_markers(body)).toContain(
			'<pre class="shiki"><code class="shiki language-typescript">',
		);
		expect(strip_ssr_markers(body)).toContain("</code></pre>");
		expect(body).not.toContain("position:fixed");
		expect(body).not.toContain("inset:0");
		expect(body).toContain('<code class="shiki language-typescript">');
		expect(body).toContain("const answer = 42;");
		expect(body).toContain('aria-label="Copy code"');
		expect(body).not.toContain("highlights=");
		expect(body).not.toContain("language=");
		expect(body).not.toContain("meta=");
	});

	it("discards directive styles from Comark's actual parsed fence attributes", async () => {
		const { parse_conversation_markdown } = await frontend_vite.ssrLoadModule(
			"/src/lib/components/markdown/parsing.ts",
		);
		const tree = await parse_conversation_markdown(
			[
				'::pre{style="position:fixed;inset:0"}',
				"```typescript[src/lib/answer.ts]{1}",
				"const answer = 42;",
				"```",
				"::",
			].join("\n"),
		);
		const attributes = tree.nodes[0][1] as Record<string, unknown>;
		const body = await RenderCodeSnippet(attributes);

		expect(attributes.style).toBe("position:fixed;inset:0");
		expect(body).toContain('class="docs-code-snippet not-prose"');
		expect(body).toContain("src/lib/answer.ts");
		expect(body).not.toContain("position:fixed");
		expect(body).not.toContain("inset:0");
	});

	it("uses the copy overlay without a filename", async () => {
		const body = await RenderCodeSnippet({});

		expect(body).toContain("docs-code-snippet-copy-overlay");
		expect(body).not.toContain("docs-code-snippet-header");
		expect(body).not.toContain("docs-code-snippet-filename");
		expect(body).toContain("docs-code-snippet-copy-overlay");
		expect(body).toContain('class="docs-code-snippet-body"');
		expect(body).toContain("<pre>");
		expect(body).toContain('role="status"');
	});

	it("keeps copy inside the SER-owned browser and clipboard boundaries", () => {
		expect(source).toContain('<script lang="ts" effect>');
		expect(source).toContain('from "$lib/browser/clipboard"');
		expect(source).toContain('from "$lib/browser/dom"');
		expect(source).toContain('yield* RunBrowserDom(() => code_element?.textContent ?? "")');
		expect(source).toContain("yield* WriteClipboardText(text)");
		expect(source).toContain("onclick={yield* CopyCode}");
		expect(source).toContain('Effect.sleep("1400 millis")');
		expect(source).toContain('if (copy_attempt === attempt) copy_state = "idle"');
		expect(source).not.toContain("navigator.clipboard");
		expect(source).not.toMatch(/Effect\.run(?:Sync|Promise|Fork)/u);
	});
});
