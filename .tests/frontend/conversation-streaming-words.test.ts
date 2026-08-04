import { Effect, Fiber, Queue } from "effect";
import { TestClock } from "effect/testing";
import { describe, expect, it } from "vitest";
import {
	conversation_streaming_words_plugin,
	count_pending_streaming_words,
	create_conversation_streaming_words_plugin,
	find_next_reveal_boundary,
	get_streaming_word_delay,
	reveal_streaming_words,
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

	it("fails closed for corrections that are not prefix appends", () => {
		const correction = streaming("Hello there");

		expect(find_next_reveal_boundary("Hello world", correction)).toBe(correction.text.length);
		expect(reveal_streaming_words("Hello world", correction)).toBe("Hello there");
	});

	it("uses bounded tiers and returns to the calm stagger cadence", () => {
		expect(count_pending_streaming_words("Hello ", streaming("Hello wide world "))).toBe(2);
		expect(count_pending_streaming_words("stale", streaming("corrected"))).toBe(0);
		expect(get_streaming_word_delay(0)).toBe(40);
		expect(get_streaming_word_delay(4)).toBe(40);
		expect(get_streaming_word_delay(5)).toBe(28);
		expect(get_streaming_word_delay(13)).toBe(18);
		expect(get_streaming_word_delay(33)).toBe(12);
		expect(get_streaming_word_delay(10_000)).toBe(12);
	});

	it("lets a newer transport target preempt an in-flight visual delay", async () => {
		const correction = streaming("corrected response");
		const outcome = await Effect.runPromise(
			Effect.gen(function* () {
				const targets = yield* Queue.sliding<StreamingWordsTarget>(1);
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
