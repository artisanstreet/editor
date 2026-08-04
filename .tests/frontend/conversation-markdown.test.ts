import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

import { parse_conversation_markdown } from "../../modules/frontend/src/lib/components/markdown/parsing";

const ReadSource = (path: string) =>
	readFileSync(resolve(import.meta.dirname, "../..", path), "utf8");

type TreeNode = string | ReadonlyArray<unknown> | null;

const collect_tags = (nodes: ReadonlyArray<TreeNode>): ReadonlyArray<string> =>
	nodes.flatMap((node) => {
		if (typeof node === "string" || node === null || !Array.isArray(node)) return [];
		const [tag, , ...children] = node;
		const child_tags = collect_tags(children as ReadonlyArray<TreeNode>);
		return typeof tag === "string" ? [tag, ...child_tags] : child_tags;
	});

describe("conversation markdown dialect", () => {
	it("parses common markdown into element nodes", async () => {
		const tree = await parse_conversation_markdown("# Title\n\nSome **bold** text.");
		const tags = collect_tags(tree.nodes);

		expect(tags).toContain("h1");
		expect(tags).toContain("p");
		expect(tags).toContain("strong");
	});

	it("auto-closes unterminated syntax so streamed frames stay renderable", async () => {
		const emphasis = await parse_conversation_markdown("streaming **bol");
		const fence = await parse_conversation_markdown("```ts\nconst partial = ");

		expect(collect_tags(emphasis.nodes)).toContain("strong");
		expect(collect_tags(fence.nodes)).toContain("pre");
	});

	it("keeps raw HTML inert instead of producing live elements", async () => {
		const tree = await parse_conversation_markdown(
			'before <img src="x" onerror="alert(1)"> <script>alert(1)</script> after',
		);
		const tags = collect_tags(tree.nodes);

		expect(tags).not.toContain("img");
		expect(tags).not.toContain("script");
		expect(JSON.stringify(tree.nodes)).toContain('<img src=\\"x\\" onerror=');
	});

	it("refuses executable link protocols during parsing", async () => {
		const tree = await parse_conversation_markdown("[click](javascript:alert(1))");

		expect(collect_tags(tree.nodes)).not.toContain("a");
		expect(JSON.stringify(tree.nodes)).not.toContain('"href"');
	});
});

describe("conversation markdown rendering", () => {
	it("renders assistant messages through the markdown renderer with lifecycle-driven streaming", () => {
		const message = ReadSource(
			"modules/frontend/src/routes/components/conversation-message.sv",
		);

		expect(message).toContain(
			'import MarkdownContent from "$lib/components/markdown/content.sv"',
		);
		expect(message).toContain(
			'<MarkdownContent streaming={item.lifecycle === "streaming"} text={item.text} />',
		);
	});

	it("uses the shared dialect, the prose foundation, and the streaming caret", () => {
		const content = ReadSource("modules/frontend/src/lib/components/markdown/content.sv");
		const stream_word = ReadSource(
			"modules/frontend/src/lib/components/markdown/stream-word.sv",
		);
		const stream_word_styles = ReadSource(
			"modules/frontend/src/lib/styles/markdown/components/streaming-word.css",
		);

		expect(content).toContain('import { conversation_parse_options } from "./parsing"');
		expect(content).toContain("options={conversation_parse_options}");
		expect(content).toContain('class="prose conversation-markdown"');
		expect(content).toContain("Queue.sliding<StreamingWordsTarget>(1)");
		expect(content).toContain('getPropertyValue("--stagger-dur")');
		expect(content.match(/yield\* wait_for_streaming_word_delay_or_target/gu)).toHaveLength(2);
		expect(content).not.toContain("yield* Effect.sleep(streaming_word_animation_duration)");
		expect(content).toContain("markdown={revealed_text}");
		expect(content).toContain("ProseStreamWord: StreamWord");
		expect(content).toContain("caret");
		expect(content).toContain("ProseA: Anchor");
		expect(content).toContain("ProseImg: Image");
		expect(content).not.toMatch(/setInterval|setTimeout|requestAnimationFrame/u);
		expect(stream_word).toContain("untrack(() => incoming)");
		expect(stream_word).toContain("onanimationend");
		expect(stream_word_styles).toContain("@keyframes docs-stream-word-in");
		expect(stream_word_styles).toContain("var(--stagger-dur, 500ms)");
		expect(stream_word_styles).toContain("prefers-reduced-motion: reduce");
	});

	it("hardens links and never auto-fetches images from assistant markdown", () => {
		const anchor = ReadSource("modules/frontend/src/lib/components/markdown/anchor.sv");
		const image = ReadSource("modules/frontend/src/lib/components/markdown/image.sv");

		expect(anchor).toContain('rel="noopener noreferrer"');
		expect(anchor).toContain('target="_blank"');
		expect(anchor).toContain('protocol === "https:"');
		expect(image).not.toContain("<img");
		expect(image).toContain("<Anchor href={src}>");
	});
});
