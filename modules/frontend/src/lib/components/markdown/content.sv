<script lang="ts" effect>
	import { Comark } from "@comark/svelte";
	import { Effect, Option, Queue } from "effect";
	import { untrack } from "svelte";
	import { RunBrowserDom } from "$lib/browser/dom";
	import Anchor from "./anchor.sv";
	import CodeSnippet from "./code-snippet.sv";
	import Image from "./image.sv";
	import {
		create_conversation_streaming_markdown_plugins,
		conversation_markdown_plugins,
	} from "./highlighting";
	import MathExpression from "./math-expression.sv";
	import MermaidDiagram from "./mermaid-diagram.sv";
	import { conversation_parse_options } from "./parsing";
	import StreamWord from "./stream-word.sv";
	import {
		count_pending_streaming_words,
		create_conversation_streaming_words_plugin,
		get_streaming_word_delay,
		is_append_only_streaming_target,
		reveal_streaming_words,
		type StreamingWordsTarget,
		wait_for_streaming_word_delay_or_target,
	} from "./streaming-words";

	let { streaming = false, text }: { streaming?: boolean; text: string } = $props();
	let revealed_text = $state(untrack(() => text));
	let presentation_settled = $state(untrack(() => !streaming));
	let animation_generation = 0;
	let pending_animation_generation = $state<number | undefined>();
	const streaming_word_targets = yield* Queue.sliding<StreamingWordsTarget>(1);
	const streaming_words_plugin = create_conversation_streaming_words_plugin(
		() => pending_animation_generation,
	);
	const conversation_streaming_markdown_plugins =
		create_conversation_streaming_markdown_plugins(streaming_words_plugin);
	const reduced_motion = yield* RunBrowserDom(() =>
		globalThis.matchMedia("(prefers-reduced-motion: reduce)").matches,
	).pipe(
		Effect.catch(() =>
			Effect.gen(function* () {
				return true;
			}),
		),
	);
	const streaming_word_animation_duration = yield* RunBrowserDom(() => {
		const value = globalThis
			.getComputedStyle(globalThis.document.documentElement)
			.getPropertyValue("--stagger-dur")
			.trim();
		const duration = Number.parseFloat(value);
		if (!Number.isFinite(duration)) return 500;
		return value.endsWith("ms") ? duration : duration * 1000;
	}).pipe(
		Effect.catch(() =>
			Effect.gen(function* () {
				return 500;
			}),
		),
	);

	const RevealStreamingWords = Effect.gen(function* () {
		let current_text = revealed_text;
		let target = yield* Queue.take(streaming_word_targets);

		while (true) {
			const latest = yield* Queue.poll(streaming_word_targets);
			if (Option.isSome(latest)) target = latest.value;
			if (target.streaming) presentation_settled = false;

			if (!is_append_only_streaming_target(current_text, target) || reduced_motion) {
				current_text = target.text;
				pending_animation_generation = undefined;
				revealed_text = target.text;
				presentation_settled = !target.streaming;
				target = yield* Queue.take(streaming_word_targets);
				continue;
			}

			const next_text = reveal_streaming_words(current_text, target);
			if (next_text === current_text) {
				if (!target.streaming && !presentation_settled) {
					const hold_outcome = yield* wait_for_streaming_word_delay_or_target(
						streaming_word_targets,
						streaming_word_animation_duration,
					);
					if (hold_outcome._tag === "Target") {
						target = hold_outcome.target;
						continue;
					}
					presentation_settled = true;
				}
				target = yield* Queue.take(streaming_word_targets);
				continue;
			}

			const backlog = count_pending_streaming_words(current_text, target);
			const delay_outcome = yield* wait_for_streaming_word_delay_or_target(
				streaming_word_targets,
				get_streaming_word_delay(backlog),
			);
			if (delay_outcome._tag === "Target") {
				target = delay_outcome.target;
				continue;
			}
			current_text = next_text;
			pending_animation_generation = ++animation_generation;
			revealed_text = next_text;
		}
	});
	/** The component scope interrupts the reveal worker when its message unmounts. */
	yield* RevealStreamingWords.pipe(Effect.forkScoped);
	yield* Queue.offer(streaming_word_targets, { streaming, text });

	const presentation_streaming = $derived(
		streaming || revealed_text !== text || !presentation_settled,
	);
	const active_plugins = $derived(
		presentation_streaming
			? conversation_streaming_markdown_plugins
			: conversation_markdown_plugins,
	);

	const components = {
		ProseA: Anchor,
		ProseImg: Image,
		ProsePre: CodeSnippet,
		ProseMath: MathExpression,
		ProseMermaid: MermaidDiagram,
		ProseStreamWord: StreamWord,
	};
</script>

<!-- Rich nodes settle once at turn completion; partial math and Mermaid remain literal/code. -->
<Comark
	class="prose conversation-markdown"
	markdown={revealed_text}
	options={conversation_parse_options}
	plugins={active_plugins}
	{components}
	streaming={presentation_streaming}
	caret
/>

<style>
	/**
	 * Conversation body text keeps the chat foreground color; the docs-derived
	 * .prose.prose foundation would otherwise dim it to muted-foreground.
	 */
	:global(.comark-content.conversation-markdown.prose) {
		--tw-prose-body: var(--foreground);
		color: var(--foreground);
	}
</style>
