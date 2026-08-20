import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { createServer, type ViteDevServer } from "vite";
import { afterAll, beforeAll, describe, expect, it } from "vitest";

import { ReadStylesheets } from "./stylesheet-source";

const workspace = resolve(import.meta.dirname, "../..");
const frontend_root = resolve(workspace, "modules/frontend");
const original_working_directory = process.cwd();
const ReadSource = (path: string) => readFileSync(resolve(workspace, path), "utf8");

let frontend_vite: ViteDevServer;

describe("conversation rich Markdown", () => {
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

	it("parses math and Mermaid while server-rendering stable rich boundaries", async () => {
		const [
			{ default: MathExpression },
			{ default: MermaidDiagram },
			{ parse_conversation_markdown },
			server,
		] = await Promise.all([
			frontend_vite.ssrLoadModule("/src/lib/components/markdown/math-expression.svelte"),
			frontend_vite.ssrLoadModule("/src/lib/components/markdown/mermaid-diagram.svelte"),
			frontend_vite.ssrLoadModule("/src/lib/components/markdown/test-parsing.ts"),
			frontend_vite.ssrLoadModule("svelte/server"),
		]);
		const markdown = [
			"Inline $E = mc^2$.",
			"",
			"$$\\sum_i x_i$$",
			"",
			"```mermaid",
			"graph LR",
			"A[Browser] --> B[Artisan]",
			"```",
		].join("\n");
		const tree = await parse_conversation_markdown(markdown);
		const inline_math = server.render(MathExpression, {
			props: { class: "math inline", content: "E = mc^2" },
		}).body;
		const block_math = server.render(MathExpression, {
			props: { class: "math block", content: String.raw`\sum_i x_i` },
		}).body;
		const mermaid_boundary = server.render(MermaidDiagram, {
			props: { content: "graph LR\nA[Browser] --> B[Artisan]" },
		}).body;
		const { render_conversation_mermaid } = await frontend_vite.ssrLoadModule(
			"/src/lib/components/markdown/mermaid-rendering.ts",
		);
		const mermaid = render_conversation_mermaid("graph LR\nA[Browser] --> B[Artisan]");

		expect(JSON.stringify(tree.nodes)).toContain('"math"');
		expect(JSON.stringify(tree.nodes)).toContain('"mermaid"');
		expect(inline_math).toContain('class="docs-math-fallback"');
		expect(inline_math).toContain('aria-busy="true"');
		expect(inline_math).toContain("E = mc^2");
		expect(block_math).toContain('class="docs-math-fallback not-prose"');
		expect(block_math).toContain("<pre");
		expect(block_math).toContain(String.raw`\sum_i x_i`);
		expect(mermaid_boundary).toContain('data-render-status="loading"');
		expect(mermaid_boundary).toContain("graph LR");
		expect(mermaid.status).toBe("rendered");
		if (mermaid.status === "rendered") {
			expect(mermaid.html).toContain("<svg");
			expect(mermaid.html).toContain("--surface:var(--card)");
			expect(mermaid.html).not.toContain("--surface:var(--surface-100)");
			expect(mermaid.html).not.toContain("fonts.googleapis.com");
		}
		expect(ReadSource("modules/frontend/src/lib/components/markdown/content.svelte")).toContain(
			"ProseMath: MathExpression",
		);
		expect(ReadSource("modules/frontend/src/lib/components/markdown/content.svelte")).toContain(
			"ProseMermaid: MermaidDiagram",
		);
	});

	it("keeps generated rich markup inside the renderer security boundary", async () => {
		const [{ render_conversation_math }, mermaid_renderer] = await Promise.all([
			frontend_vite.ssrLoadModule("/src/lib/components/markdown/math-rendering.ts"),
			frontend_vite.ssrLoadModule("/src/lib/components/markdown/mermaid-rendering.ts"),
		]);
		const { is_safe_conversation_mermaid_svg, render_conversation_mermaid } = mermaid_renderer;
		const math = render_conversation_math(String.raw`\href{javascript:alert(1)}{bad}`, false);
		const mermaid = render_conversation_mermaid(
			'graph LR\nA["</text><script>alert(1)</script>"] --> B',
		);

		expect(math.status).toBe("rendered");
		if (math.status === "rendered") expect(math.html).not.toContain("href=");
		expect(mermaid.status).toBe("rendered");
		if (mermaid.status === "rendered") {
			expect(mermaid.html).not.toContain("<script");
			expect(mermaid.html).toContain("&lt;script&gt;");
			expect(mermaid.html).not.toContain("fonts.googleapis.com");
		}

		const active_svg = render_conversation_mermaid(
			"graph LR\nA --> B\nstyle A fill:url(javascript:alert(1))",
		);
		expect(active_svg.status).toBe("invalid");
		expect(
			is_safe_conversation_mermaid_svg(
				'<svg xmlns="http://www.w3.org/2000/svg"><rect fill="u\\72l(https://example.test/a)" /></svg>',
			),
		).toBe(false);
		expect(
			is_safe_conversation_mermaid_svg(
				'<svg xmlns="http://www.w3.org/2000/svg"><style>@import "//example.test/a";</style></svg>',
			),
		).toBe(false);
		expect(
			is_safe_conversation_mermaid_svg(
				'<svg xmlns="http://www.w3.org/2000/svg"><foreignObject onload="alert(1)" /></svg>',
			),
		).toBe(false);
		expect(
			is_safe_conversation_mermaid_svg(
				'<svg xmlns="http://www.w3.org/2000/svg"><image href="data:image/svg+xml,bad" /></svg>',
			),
		).toBe(false);
	});

	it("accepts the Mermaid diagram families supported by the renderer", async () => {
		const { render_conversation_mermaid } = await frontend_vite.ssrLoadModule(
			"/src/lib/components/markdown/mermaid-rendering.ts",
		);
		const diagrams = [
			"graph LR\nA[Browser] --> B[Portless :443]\nB --> C[(Map store)]",
			"sequenceDiagram\nAlice->>Bob: Hello",
			"classDiagram\nclass Animal\nAnimal : +String name",
			"erDiagram\nCUSTOMER ||--o{ ORDER : places",
			'xychart-beta\nx-axis [1, 2, 3]\ny-axis "Sales" 0 --> 10\nline [2, 4, 6]',
			"stateDiagram-v2\n[*] --> Idle",
		];

		for (const source of diagrams) {
			expect(render_conversation_mermaid(source).status, source).toBe("rendered");
		}
	});

	it("defers unstable math and Mermaid until a streaming turn completes", () => {
		const content = ReadSource("modules/frontend/src/lib/components/markdown/content.svelte");
		const highlighting = ReadSource(
			"modules/frontend/src/lib/components/markdown/highlighting.ts",
		);
		const settled_highlighting = ReadSource(
			"modules/frontend/src/lib/components/markdown/settled-highlighting.ts",
		);
		const mermaid_renderer = ReadSource(
			"modules/frontend/src/lib/components/markdown/mermaid-renderer.svelte",
		);
		const math_expression = ReadSource(
			"modules/frontend/src/lib/components/markdown/math-expression.svelte",
		);
		const math_renderer = ReadSource(
			"modules/frontend/src/lib/components/markdown/math-renderer.svelte",
		);

		/** The streaming plugin sets never carry math or Mermaid recognition. */
		expect(content).toContain("? [streaming_words_plugin]");
		expect(content).toContain(": [streaming_highlight_plugin, streaming_words_plugin]");
		expect(content).toContain("create_conversation_streaming_words_plugin");
		/**
		 * Mid-stream highlighting is safe only because it is bounded: the
		 * incremental parse re-emits just the tail, the one unterminated fence
		 * stays plain instead of re-tokenizing as it grows, and every closed
		 * fence body tokenizes once into a shared cache that the settle reparse
		 * reuses.
		 */
		expect(highlighting).toContain("LoadConversationStreamingHighlightPlugin");
		expect(settled_highlighting).toContain("is_open_conversation_fence_body");
		expect(settled_highlighting).toContain("fence_token_cache");
		expect(content).toContain("yield* LoadConversationSettledMarkdownPlugins(");
		expect(highlighting).toContain('try: () => import("./settled-highlighting")');
		expect(highlighting).not.toContain('from "shiki/');
		expect(settled_highlighting).toContain('from "shiki/');
		expect(settled_highlighting).not.toMatch(/import\s+\w+\s+from\s+"shiki\/dist\/langs\//u);
		expect(settled_highlighting).toContain("registerDefaultLanguages: false");
		expect(settled_highlighting).toContain(
			'javascript: () => import("shiki/dist/langs/javascript.mjs")',
		);
		expect(math_expression).not.toContain('from "./math-rendering"');
		expect(math_renderer).toContain("yield* MathRendererController");
		expect(math_renderer).toContain("renderer_controller.Current");
		expect(math_renderer).toContain("renderer_controller.Refresh.pipe(Effect.forkScoped)");
		expect(content).not.toContain("katex");
		expect(mermaid_renderer).toContain("Effect.tryPromise");
		expect(mermaid_renderer).toContain("MermaidRendererLoadFailure");
		expect(mermaid_renderer).not.toContain("Effect.promise");
	});

	it("keeps a settled transcript's first paint off the highlighter's chunk graph", () => {
		const content = ReadSource("modules/frontend/src/lib/components/markdown/content.svelte");
		const layout = ReadSource("modules/frontend/src/routes/+layout.svelte");
		const warming = ReadSource("modules/frontend/src/lib/components/markdown/warming.ts");

		/**
		 * The settled plugin set upgrades in the background: the transcript paints
		 * with unhighlighted code and the worker swaps Shiki in when it arrives. A
		 * direct blocking yield here held every first-of-session code message —
		 * and the whole thread open behind it — until Shiki's ~2MB chunk graph
		 * loaded.
		 */
		expect(content).toContain("TryResidentConversationSettledMarkdownPlugins(source)");
		expect(content).toContain(
			"yield* RenderSettledTree(source, conversation_rich_markdown_plugins)",
		);
		expect(content).toContain("yield* RenderSettledMarkdown.pipe(Effect.forkScoped)");
		/** The shell warms renderer chunks at idle so a first settled message rarely pays them. */
		expect(warming).toContain('() => import("./settled-highlighting")');
		expect(warming).toContain("requestIdleCallback");
		expect(layout).toContain("yield* WarmConversationRenderers.pipe(Effect.forkScoped)");
	});

	it("selects only requested known fence grammars before loading settled Shiki", async () => {
		const { RequestedConversationFenceLanguages } = await frontend_vite.ssrLoadModule(
			"/src/lib/components/markdown/highlighting.ts",
		);

		expect(RequestedConversationFenceLanguages("```ts\nconst answer = 42\n```")).toEqual([
			"typescript",
		]);
		expect(
			RequestedConversationFenceLanguages(
				"```typescript[src/lib/editor.ts]{1}\nexport {}\n```",
			),
		).toEqual(["typescript"]);
		expect(RequestedConversationFenceLanguages("```unknown-language\nplain text\n```")).toEqual(
			[],
		);
	});

	it("registers every grammar when different messages settle concurrently", async () => {
		const [
			effect_module,
			{ LoadConversationSettledMarkdownPlugins, TokenizeConversationFences },
			{ parse },
			{ resetHighlighter },
		] = await Promise.all([
			frontend_vite.ssrLoadModule("effect"),
			frontend_vite.ssrLoadModule("/src/lib/components/markdown/settled-highlighting.ts"),
			frontend_vite.ssrLoadModule("@comark/svelte/parse"),
			frontend_vite.ssrLoadModule("comark/plugins/highlight"),
		]);
		const { Effect } = effect_module;
		resetHighlighter();
		const [typescript_plugins, python_plugins] = await Effect.runPromise(
			Effect.all(
				[
					LoadConversationSettledMarkdownPlugins(["typescript"]),
					LoadConversationSettledMarkdownPlugins(["python"]),
				] as const,
				{ concurrency: "unbounded" },
			),
		);
		/**
		 * Tokens come from the shared fence cache; a grammar that lost the
		 * concurrent registration race would tokenize to plain text here.
		 */
		await Effect.runPromise(
			Effect.all(
				[
					TokenizeConversationFences([
						{ body: "const answer: number = 42", language: "typescript" },
					]),
					TokenizeConversationFences([{ body: "answer: int = 42", language: "python" }]),
				] as const,
				{ concurrency: "unbounded" },
			),
		);
		const [typescript_tree, python_tree] = await Promise.all([
			parse("```typescript\nconst answer: number = 42\n```", {
				plugins: typescript_plugins,
			}),
			parse("```python\nanswer: int = 42\n```", { plugins: python_plugins }),
		]);

		expect(JSON.stringify(typescript_tree.nodes)).toContain('"span"');
		expect(JSON.stringify(python_tree.nodes)).toContain('"span"');
	});

	it("makes code cards fill the available transcript width without wrapping long code", () => {
		const styles = ReadStylesheets();

		expect(styles).toMatch(/\.prose \.docs-code-snippet \{[\s\S]*w-full max-w-full/u);
		expect(styles).toMatch(/\.docs-code-snippet-body \.shiki \{[\s\S]*min-w-full w-max/u);
		/** Media sizes itself from the column rather than from a media-only token. */
		expect(styles).not.toContain("width: var(--docs-prose-media-inline-size:");
	});
});
