import type { ComarkElement, ComarkNode, ComarkPlugin } from "comark";
import { Effect, Queue } from "effect";

/** The latest transport payload used by the local word-reveal queue. */
export type StreamingWordsTarget = {
	readonly text: string;
	readonly streaming: boolean;
};

export type StreamingWordDelayOutcome =
	| { readonly _tag: "Elapsed" }
	| { readonly _tag: "Target"; readonly target: StreamingWordsTarget };

const calm_delay_ms = 40;
const medium_delay_ms = 28;
const fast_delay_ms = 18;
const catch_up_delay_ms = 12;

const is_whitespace = (value: string): boolean => /^\s+$/u.test(value);

const is_comark_element = (node: ComarkNode): node is ComarkElement =>
	Array.isArray(node) && typeof node[0] === "string";

const is_excluded_streaming_subtree = (tag: string): boolean =>
	["code", "math", "mermaid", "pre", "stream-word"].includes(tag.toLowerCase());

/**
 * Whether a target can safely extend the already queued prefix. Corrections
 * deliberately fail closed so they appear immediately rather than animating a
 * stale branch of the response.
 */
export const is_append_only_streaming_target = (
	current_prefix: string,
	target: StreamingWordsTarget,
): boolean => target.text.startsWith(current_prefix);

/**
 * Finds the next byte boundary the reveal queue may render. Streaming targets
 * retain a final unterminated word; settled targets expose every final byte.
 */
export const find_next_reveal_boundary = (
	current_prefix: string,
	target: StreamingWordsTarget,
): number => {
	if (!is_append_only_streaming_target(current_prefix, target)) return target.text.length;

	const target_text = target.text;
	const trailing_word = target_text.match(/\S+$/u);
	const safe_end =
		target.streaming && trailing_word
			? target_text.length - trailing_word[0].length
			: target_text.length;

	if (current_prefix.length >= safe_end) return current_prefix.length;

	let current_word_start = current_prefix.length;
	while (current_word_start > 0 && !is_whitespace(current_prefix[current_word_start - 1] ?? "")) {
		current_word_start -= 1;
	}
	const next_whitespace = target_text.slice(current_word_start).search(/\s/u);

	if (next_whitespace === -1) return safe_end;

	let boundary = current_word_start + next_whitespace;
	while (boundary < safe_end && /\s/u.test(target_text[boundary] ?? "")) boundary += 1;
	return boundary;
};

/** Returns the exact target prefix permitted by the next queue tick. */
export const reveal_streaming_words = (
	current_prefix: string,
	target: StreamingWordsTarget,
): string => target.text.slice(0, find_next_reveal_boundary(current_prefix, target));

/** Counts pending visual words so the queue can select its catch-up tier. */
export const count_pending_streaming_words = (
	current_prefix: string,
	target: StreamingWordsTarget,
): number => {
	if (!is_append_only_streaming_target(current_prefix, target)) return 0;
	return target.text.slice(current_prefix.length).match(/\S+/gu)?.length ?? 0;
};

/**
 * The queue starts at transitions.dev's 40ms stagger cadence, then reduces
 * only its delay as the backlog grows. Every tier is bounded and it returns to
 * calm immediately once the reader catches up.
 */
export const get_streaming_word_delay = (backlog_words: number): number => {
	if (backlog_words <= 4) return calm_delay_ms;
	if (backlog_words <= 12) return medium_delay_ms;
	if (backlog_words <= 32) return fast_delay_ms;
	return catch_up_delay_ms;
};

/**
 * A fresh transport target interrupts the visual cadence. Corrections therefore
 * fail closed before a stale queued word can be committed, while a newer append
 * is reconsidered against the latest sliding-queue value.
 */
export const wait_for_streaming_word_delay_or_target = (
	targets: Queue.Dequeue<StreamingWordsTarget>,
	delay_ms: number,
): Effect.Effect<StreamingWordDelayOutcome> =>
	Effect.gen(function* () {
		return yield* Effect.raceFirst(
			Effect.gen(function* () {
				yield* Effect.sleep(delay_ms);
				return { _tag: "Elapsed" } as const;
			}),
			Effect.gen(function* () {
				const target = yield* Queue.take(targets);
				return { _tag: "Target", target } as const;
			}),
		);
	});

const wrap_streaming_text = (text: string, last_word: { value?: ComarkElement }): ComarkNode[] => {
	const nodes: ComarkNode[] = [];

	for (const part of text.split(/(\s+)/u)) {
		if (part.length === 0) continue;
		if (is_whitespace(part)) {
			nodes.push(part);
			continue;
		}

		const word: ComarkElement = ["stream-word", { incoming: false }, part];
		last_word.value = word;
		nodes.push(word);
	}

	return nodes;
};

const wrap_streaming_node = (
	node: ComarkNode,
	last_word: { value?: ComarkElement },
): ComarkNode[] => {
	if (typeof node === "string") return wrap_streaming_text(node, last_word);
	if (!is_comark_element(node) || is_excluded_streaming_subtree(node[0])) return [node];

	const [tag, attributes, ...children] = node;
	const wrapped_children = children.flatMap((child) => wrap_streaming_node(child, last_word));
	return [[tag, attributes, ...wrapped_children]];
};

/**
 * Pure AST rewrite used by the streaming-only Comark plugin. Whitespace stays
 * as raw text, so parser and renderer byte boundaries remain intact.
 */
export const wrap_streaming_words = (
	nodes: readonly ComarkNode[],
	animate_latest_word = true,
): ComarkNode[] => {
	const last_word: { value?: ComarkElement } = {};
	const wrapped_nodes = nodes.flatMap((node) => wrap_streaming_node(node, last_word));

	if (animate_latest_word && last_word.value) last_word.value[1].incoming = true;
	return wrapped_nodes;
};

/**
 * Makes animation generations single-use within one message. If Comark reparses
 * the same generation under a different Markdown parent, the remounted word is
 * wrapped for layout but does not replay its entrance.
 */
export const create_conversation_streaming_words_plugin = (
	get_animation_generation: () => number | undefined,
): ComarkPlugin => {
	let consumed_generation: number | undefined;

	return {
		name: "conversation-streaming-words",
		post: (state) => {
			const generation = get_animation_generation();
			const animate_latest_word =
				generation !== undefined && generation !== consumed_generation;
			state.tree.nodes = wrap_streaming_words(state.tree.nodes, animate_latest_word);
			if (animate_latest_word) consumed_generation = generation;
		},
	};
};

/** Synchronous default for pure plugin tests and non-conversation consumers. */
export const conversation_streaming_words_plugin: ComarkPlugin =
	create_conversation_streaming_words_plugin(() => 0);
