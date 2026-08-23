import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { Effect, Fiber, Queue } from "effect";
import { TestClock } from "effect/testing";
import { describe, expect, it } from "vitest";
import {
	conversation_streaming_words_plugin,
	count_pending_streaming_words,
	create_conversation_streaming_words_plugin,
	find_next_reveal_boundary,
	get_streaming_word_pacing,
	reveal_streaming_words,
	should_animate_streaming_target,
	wait_for_streaming_word_delay_or_target,
	wrap_streaming_words,
	type StreamingWordsTarget,
} from "../../modules/frontend/src/lib/components/markdown/streaming-words";

const streaming = (text: string): StreamingWordsTarget => ({ streaming: true, text });
const settled = (text: string): StreamingWordsTarget => ({ streaming: false, text });
type StreamingWordsPluginState = Parameters<
	NonNullable<typeof conversation_streaming_words_plugin.post>
>[0];
type ComarkTree = StreamingWordsPluginState["tree"];
type ComarkNode = ComarkTree["nodes"][number];

const get_word_nodes = (nodes: readonly ComarkNode[]): [string, boolean][] => {
	const words: [string, boolean][] = [];
	for (const node of nodes) {
		if (!Array.isArray(node) || typeof node[0] !== "string") continue;
		if (node[0] === "stream-word") {
			words.push([String(node[2] ?? ""), node[1].incoming === true]);
			continue;
		}
		words.push(...get_word_nodes(node.slice(2) as readonly ComarkNode[]));
	}
	return words;
};

describe("conversation streaming words", () => {
	it("paints exact text while the first asynchronous Markdown tree is pending", () => {
		const source = readFileSync(
			resolve("modules/frontend/src/lib/components/markdown/content.svelte"),
			"utf8",
		);

		expect(source).toContain("{:else if revealed_text.trim().length > 0}");
		expect(source).toContain('<p class="whitespace-pre-wrap">{revealed_text}</p>');
	});

	it("enables word reveal only for a live owning run and turn", () => {
		const workspace = readFileSync(
			resolve("modules/frontend/src/routes/components/thread-workspace.svelte"),
			"utf8",
		);
		const message = readFileSync(
			resolve("modules/frontend/src/routes/components/conversation-message.svelte"),
			"utf8",
		);

		expect(workspace).toContain("const streaming_turn_ids = $derived(");
		expect(workspace).toContain("message_streaming={run_active &&");
		expect(workspace).toContain("block.item.run_id === active_run_id");
		expect(workspace).toContain("streaming_turn_ids.has(block.turn_id)");
		expect(message).toContain(
			'streaming={message_streaming && item.lifecycle === "streaming"}',
		);
		expect(message).not.toContain(
			'<MarkdownContent streaming={item.lifecycle === "streaming"}',
		);
	});

	it("preserves every byte while revealing complete streaming words", () => {
		const target = streaming("Hello,\tworld  \nnext");

		expect(find_next_reveal_boundary("", target)).toBe(7);
		expect(reveal_streaming_words("", target)).toBe("Hello,\t");
		expect(reveal_streaming_words("Hello,\t", target)).toBe("Hello,\tworld  \n");
	});

	it("holds an extending partial token until its trailing boundary arrives", () => {
		expect(reveal_streaming_words("Hel", streaming("Hello"))).toBe("Hel");
		expect(reveal_streaming_words("Hel", streaming("Hello "))).toBe("Hello ");
		expect(reveal_streaming_words("Hello ", streaming("Hello world"))).toBe("Hello ");
	});

	it("releases the final word when the target settles", () => {
		expect(reveal_streaming_words("Hello ", settled("Hello world"))).toBe("Hello world");
		expect(find_next_reveal_boundary("Hello world", settled("Hello world"))).toBe(
			"Hello world".length,
		);
	});

	it("snaps settled hydration while still draining a genuinely live presentation", () => {
		expect(should_animate_streaming_target(true, settled("hydrated history"))).toBe(false);
		expect(should_animate_streaming_target(false, settled("finished live reply"))).toBe(true);
		expect(should_animate_streaming_target(true, streaming("live reply"))).toBe(true);
	});

	it("fails closed for corrections that are not prefix appends", () => {
		const correction = streaming("Hello there");

		expect(find_next_reveal_boundary("Hello world", correction)).toBe(correction.text.length);
		expect(reveal_streaming_words("Hello world", correction)).toBe("Hello there");
	});

	it("uses bounded tiers and returns to the calm stagger cadence", () => {
		expect(count_pending_streaming_words("Hello ", streaming("Hello wide world "))).toBe(2);
		expect(count_pending_streaming_words("stale", streaming("corrected"))).toBe(0);
		expect(get_streaming_word_pacing(0)).toEqual({ delay_ms: 40, words: 1 });
		expect(get_streaming_word_pacing(4)).toEqual({ delay_ms: 40, words: 1 });
		expect(get_streaming_word_pacing(5)).toEqual({ delay_ms: 28, words: 1 });
		expect(get_streaming_word_pacing(13)).toEqual({ delay_ms: 20, words: 2 });
		expect(get_streaming_word_pacing(33)).toEqual({ delay_ms: 16, words: 4 });
		expect(get_streaming_word_pacing(10_000)).toEqual({ delay_ms: 16, words: 8 });
	});

	it("never ticks faster than a display frame; backlog widens the tick instead", () => {
		for (const backlog of [0, 5, 13, 33, 97, 10_000]) {
			expect(get_streaming_word_pacing(backlog).delay_ms).toBeGreaterThanOrEqual(16);
		}
	});

	it("reveals multiple words per tick while holding the unterminated tail", () => {
		const target = streaming("one two three four five");

		expect(reveal_streaming_words("", target, 3)).toBe("one two three ");
		expect(reveal_streaming_words("one ", target, 8)).toBe("one two three four ");
		expect(reveal_streaming_words("", settled("one two"), 5)).toBe("one two");
	});

	it("lets a newer transport target preempt an in-flight visual delay", async () => {
		const correction = streaming("corrected response");
		const outcome = await Effect.runPromise(
			Effect.gen(function* () {
				const targets = yield* Queue.unbounded<StreamingWordsTarget>();
				const waiting = yield* wait_for_streaming_word_delay_or_target(targets, 1_000).pipe(
					Effect.forkChild,
				);
				yield* Effect.yieldNow;
				yield* Queue.offer(targets, correction);
				return yield* Fiber.join(waiting);
			}).pipe(Effect.provide(TestClock.layer())),
		);

		expect(outcome).toEqual({ _tag: "Target", target: correction });
	});

	it("wraps nested emphasis and link text while preserving whitespace", () => {
		const wrapped = wrap_streaming_words([
			["p", {}, ["em", {}, "soft words"], " ", ["a", { href: "/" }, "linked text"]],
		]);

		expect(wrapped).toEqual([
			[
				"p",
				{},
				[
					"em",
					{},
					["stream-word", { incoming: false }, "soft"],
					" ",
					["stream-word", { incoming: false }, "words"],
				],
				" ",
				[
					"a",
					{ href: "/" },
					["stream-word", { incoming: false }, "linked"],
					" ",
					["stream-word", { incoming: true }, "text"],
				],
			],
		]);
		expect(get_word_nodes(wrapped)).toEqual([
			["soft", false],
			["words", false],
			["linked", false],
			["text", true],
		]);
	});

	it("skips code, math, Mermaid, and existing stream-word subtrees", () => {
		const source: ComarkNode[] = [
			["pre", {}, ["code", {}, "const untouched = true"]],
			["math", {}, "E = mc^2"],
			["mermaid", {}, "graph LR"],
			["stream-word", { incoming: false }, "already wrapped"],
			["p", {}, "ordinary text"],
		];

		expect(wrap_streaming_words(source)).toEqual([
			["pre", {}, ["code", {}, "const untouched = true"]],
			["math", {}, "E = mc^2"],
			["mermaid", {}, "graph LR"],
			["stream-word", { incoming: false }, "already wrapped"],
			[
				"p",
				{},
				["stream-word", { incoming: false }, "ordinary"],
				" ",
				["stream-word", { incoming: true }, "text"],
			],
		]);
	});

	it("exposes the same synchronous rewrite through the Comark plugin", () => {
		const tree: ComarkTree = { frontmatter: {}, meta: {}, nodes: [["p", {}, "plugin text"]] };
		const post = conversation_streaming_words_plugin.post;

		if (!post) throw new Error("streaming words plugin requires a post hook");
		post({ markdown: "plugin text", options: {}, tokens: [], tree });

		expect(tree.nodes).toEqual([
			[
				"p",
				{},
				["stream-word", { incoming: false }, "plugin"],
				" ",
				["stream-word", { incoming: true }, "text"],
			],
		]);
	});

	it("wraps only freshly parsed nodes when a streaming parse reuses a prefix", () => {
		const plugin = create_conversation_streaming_words_plugin(() => 7);
		const post = plugin.post;
		if (!post) throw new Error("streaming words plugin requires a post hook");

		const reused_paragraph: ComarkNode = [
			"p",
			{},
			["stream-word", { incoming: false }, "settled"],
		];
		const tree: ComarkTree = {
			frontmatter: {},
			meta: {},
			nodes: [reused_paragraph, ["p", {}, "fresh words"]],
		};
		post({
			markdown: "fresh words",
			options: {},
			reusableNodes: [reused_paragraph],
			tokens: [],
			tree,
		});

		expect(tree.nodes[0]).toBe(reused_paragraph);
		expect(get_word_nodes(tree.nodes)).toEqual([
			["settled", false],
			["fresh", false],
			["words", true],
		]);
	});

	it("consumes each animation generation once across Markdown reparses", () => {
		let generation: number | undefined = 1;
		const plugin = create_conversation_streaming_words_plugin(() => generation);
		const post = plugin.post;
		if (!post) throw new Error("streaming words plugin requires a post hook");

		const first_tree: ComarkTree = {
			frontmatter: {},
			meta: {},
			nodes: [["p", {}, "stable word"]],
		};
		post({ markdown: "stable word", options: {}, tokens: [], tree: first_tree });
		expect(get_word_nodes(first_tree.nodes)).toEqual([
			["stable", false],
			["word", true],
		]);

		const restructured_tree: ComarkTree = {
			frontmatter: {},
			meta: {},
			nodes: [["p", {}, ["em", {}, "stable word"]]],
		};
		post({ markdown: "*stable word*", options: {}, tokens: [], tree: restructured_tree });
		expect(get_word_nodes(restructured_tree.nodes)).toEqual([
			["stable", false],
			["word", false],
		]);

		generation = 2;
		const next_tree: ComarkTree = {
			frontmatter: {},
			meta: {},
			nodes: [["p", {}, ["em", {}, "stable word arrives"]]],
		};
		post({ markdown: "*stable word arrives*", options: {}, tokens: [], tree: next_tree });
		expect(get_word_nodes(next_tree.nodes)).toEqual([
			["stable", false],
			["word", false],
			["arrives", true],
		]);
	});
});
