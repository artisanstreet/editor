import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

import { parse_conversation_markdown } from "../../modules/frontend/src/lib/components/markdown/test-parsing";

import { ReadStylesheets } from "./stylesheet-source";

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
	it("renders assistant messages with owning-run and lifecycle streaming gates", () => {
		const message = ReadSource(
			"modules/frontend/src/routes/components/conversation-message.svelte",
		);

		expect(message).toContain(
			'import MarkdownContent from "$lib/components/markdown/content.svelte"',
		);
		expect(message).toContain(
			'streaming={message_streaming && item.lifecycle === "streaming"}',
		);
	});

	it("uses the shared dialect and the prose foundation without a streaming caret", () => {
		const content = ReadSource("modules/frontend/src/lib/components/markdown/content.svelte");
		const stream_word = ReadSource(
			"modules/frontend/src/lib/components/markdown/stream-word.svelte",
		);
		const stream_word_styles = ReadStylesheets();

		expect(content).toContain('import { conversation_parse_options } from "./parsing"');
		expect(content).toContain("...conversation_parse_options,");
		expect(content).toContain("prose conversation-markdown ");
		expect(content).toContain("Queue.unbounded<StreamingWordsTarget>()");
		expect(content).toContain('getPropertyValue("--stagger-dur")');
		/**
		 * The word cadence commits before it paces. Racing the tier delay against
		 * the next transport target discarded the computed word whenever a delta
		 * won, which mid-stream stalled the reveal until the provider paused; only
		 * the post-settle hold still races.
		 */
		expect(content.match(/yield\* wait_for_streaming_word_delay_or_target/gu)).toHaveLength(1);
		expect(content).toContain("yield* Effect.sleep(pacing.delay_ms)");
		/**
		 * Streaming reveals parse incrementally: the component owns one parser
		 * whose previous output lets an append-only tick re-parse only the
		 * message tail. A per-tick full reparse is what made long answers lag
		 * quadratically with their own length.
		 */
		expect(content).toContain("createParse");
		expect(content).toContain("{ streaming: true }");
		expect(content).toContain("tree={visible_tree}");
		expect(content).toContain("rendered_tree ?? empty_tree");
		expect(content).toContain("ProseStreamWord: StreamWord");
		/** A blinking pipe is not part of the reveal; the incoming word is the cue. */
		expect(content).not.toMatch(/^\s*caret\s*$/mu);
		/**
		 * `pretty` and `balance` re-break settled lines as the block grows, sliding
		 * landed words between lines while the newest one animates.
		 */
		expect(content).toContain("conversation-markdown-streaming");
		expect(ReadStylesheets()).toContain("text-wrap: wrap;");
		expect(content).toContain("ProseA: Anchor");
		expect(content).toContain("ProseImg: Image");
		expect(content).not.toMatch(/setInterval|setTimeout|requestAnimationFrame/u);
		expect(stream_word).toContain("untrack(() => incoming)");
		expect(stream_word).toContain("onanimationend");
		expect(stream_word_styles).toContain("@keyframes docs-stream-word-in");
		expect(stream_word_styles).toContain("prefers-reduced-motion: reduce");
		/**
		 * The entrance fires per word at a 12–40ms cadence, so it carries its own
		 * duration rather than the 500ms sidebar stagger, which put the whole
		 * visible tail in motion at once. The paint hint only holds each word's
		 * layer open after the animation it was meant to prepare has finished.
		 */
		expect(stream_word_styles).toContain("var(--stream-word-dur, 320ms)");
		expect(stream_word_styles).not.toContain("--stagger-dur");
		/** Scoped to the streaming-word section: the core has other, legitimate hints. */
		expect(stream_word_styles).not.toMatch(/streaming-word\.css[^─]*?will-change/u);
	});

	it("hardens links and never auto-fetches images from assistant markdown", () => {
		const anchor = ReadSource("modules/frontend/src/lib/components/markdown/anchor.svelte");
		const image = ReadSource("modules/frontend/src/lib/components/markdown/image.svelte");

		expect(anchor).toContain('rel="noopener noreferrer"');
		expect(anchor).toContain('target="_blank"');
		expect(anchor).toContain('protocol === "https:"');
		expect(image).not.toContain("<img");
		expect(image).toContain("<Anchor href={src}>");
	});
});
