import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { createServer, type ViteDevServer } from "vite";
import { afterAll, beforeAll, describe, expect, it } from "vitest";

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
			frontend_vite.ssrLoadModule("/src/lib/components/markdown/math-expression.sv"),
			frontend_vite.ssrLoadModule("/src/lib/components/markdown/mermaid-diagram.sv"),
			frontend_vite.ssrLoadModule("/src/lib/components/markdown/parsing.ts"),
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
		expect(inline_math).toContain('class="docs-math-inline"');
		expect(inline_math).toContain('class="katex"');
		expect(block_math).toContain('class="docs-math-block not-prose"');
		expect(block_math).toContain('class="katex-display"');
		expect(mermaid_boundary).toContain('data-render-status="loading"');
		expect(mermaid_boundary).toContain("graph LR");
		expect(mermaid.status).toBe("rendered");
		if (mermaid.status === "rendered") {
			expect(mermaid.html).toContain("<svg");
			expect(mermaid.html).not.toContain("fonts.googleapis.com");
		}
		expect(ReadSource("modules/frontend/src/lib/components/markdown/content.sv")).toContain(
			"ProseMath: MathExpression",
		);
		expect(ReadSource("modules/frontend/src/lib/components/markdown/content.sv")).toContain(
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
		const content = ReadSource("modules/frontend/src/lib/components/markdown/content.sv");
		const highlighting = ReadSource(
			"modules/frontend/src/lib/components/markdown/highlighting.ts",
		);
		const mermaid_renderer = ReadSource(
			"modules/frontend/src/lib/components/markdown/mermaid-renderer.sv",
		);

		expect(content).toContain(
			"streaming ? conversation_streaming_markdown_plugins : conversation_markdown_plugins",
		);
		expect(highlighting).toContain(
			"conversation_streaming_markdown_plugins = [conversation_highlight_plugin]",
		);
		expect(mermaid_renderer).toContain("Effect.tryPromise");
		expect(mermaid_renderer).toContain("MermaidRendererLoadFailure");
		expect(mermaid_renderer).not.toContain("Effect.promise");
	});

	it("makes code cards fill the available transcript width without wrapping long code", () => {
		const styles = ReadSource(
			"modules/frontend/src/lib/styles/markdown/components/code-snippet.css",
		);

		expect(styles).toMatch(/\.prose \.docs-code-snippet \{[\s\S]*w-full max-w-full/u);
		expect(styles).toMatch(/\.docs-code-snippet-body \.shiki \{[\s\S]*min-w-full w-max/u);
		expect(styles).not.toContain("width: var(--docs-prose-media-inline-size");
	});
});
