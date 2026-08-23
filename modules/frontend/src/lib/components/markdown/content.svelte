<script lang="ts" effect>
	import { ComarkRenderer } from "@comark/svelte";
	import {
		createParse,
		type ComarkParseFn,
		type ComarkPlugin,
		type ComarkTree,
	} from "comark";
	import { Data, Effect, Option, Queue } from "effect";
	import { untrack } from "svelte";
	import { RunBrowserDom } from "$lib/browser/dom";
	import Anchor from "./anchor.svelte";
	import CodeSnippet from "./code-snippet.svelte";
	import Image from "./image.svelte";
	import {
		conversation_rich_markdown_plugins,
		HasPendingConversationFenceTokenization,
		LoadConversationSettledMarkdownPlugins,
		LoadConversationStreamingHighlightPlugin,
		OpenConversationFenceBody,
		RequestedConversationFenceLanguages,
		TokenizePendingConversationFences,
		TryResidentConversationSettledMarkdownPlugins,
		WarmConversationFenceTokens,
	} from "./highlighting";
	import MathExpression from "./math-expression.svelte";
	import MermaidDiagram from "./mermaid-diagram.svelte";
	import { conversation_parse_options } from "./parsing";
	import StreamWord from "./stream-word.svelte";
	import {
		count_pending_streaming_words,
		create_conversation_streaming_words_plugin,
		get_streaming_word_pacing,
		is_append_only_streaming_target,
		reveal_streaming_words,
		should_animate_streaming_target,
		type StreamingWordsTarget,
		wait_for_streaming_word_delay_or_target,
	} from "./streaming-words";

	class ConversationMarkdownParseFailure extends Data.TaggedError(
		"ConversationMarkdownParseFailure",
	)<{
		readonly cause: unknown;
	}> {}

	let { streaming = false, text }: { streaming?: boolean; text: string } = $props();
	let revealed_text = $state(untrack(() => text));
	let presentation_settled = $state(untrack(() => !streaming));
	let animation_generation = 0;
	let pending_animation_generation = $state<number | undefined>();
	let rendered_tree = $state.raw<ComarkTree | undefined>();
	let settled_transform_pending = $state(
		untrack(() => !streaming && text.trim().length > 0),
	);
	let settled_transform_generation = 0;
	/** Keeps an already-mounted Comark child valid while its owning message is being replaced. */
	const empty_tree: ComarkTree = { frontmatter: {}, meta: {}, nodes: [] };
	const visible_tree = $derived(rendered_tree ?? empty_tree);
	const streaming_word_targets = yield* Queue.unbounded<StreamingWordsTarget>();
	const streaming_words_plugin = create_conversation_streaming_words_plugin(
		() => pending_animation_generation,
	);
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

	/**
	 * The open fence body of the text most recently handed to the parser. The
	 * fence-highlight plugin reads it to leave exactly that one block plain —
	 * re-tokenising a block that grows with every revealed word is the cost
	 * this pipeline retires.
	 */
	const open_fence_body: { current: string | undefined } = { current: undefined };
	const has_conversation_fences = (value: string): boolean =>
		value.includes("```") || value.includes("~~~");

	let streaming_highlight_plugin: ComarkPlugin | undefined;
	const MakeStreamingParser = (): ComarkParseFn =>
		createParse({
			...conversation_parse_options,
			plugins:
				streaming_highlight_plugin === undefined
					? [streaming_words_plugin]
					: [streaming_highlight_plugin, streaming_words_plugin],
		});
	let streaming_parser = MakeStreamingParser();
	let streaming_parser_generation = 0;
	let parsed_text: string | undefined;
	let parsed_parser_generation = -1;
	let latest_streaming_target: StreamingWordsTarget | undefined;

	const ParseConversationMarkdown = (
		parser: ComarkParseFn,
		source: string,
		streaming_parse: boolean,
	) =>
		Effect.gen(function* () {
			open_fence_body.current =
				streaming_parse && has_conversation_fences(source)
					? OpenConversationFenceBody(source)
					: undefined;
			return yield* Effect.tryPromise({
				catch: (cause) => new ConversationMarkdownParseFailure({ cause }),
				try: () =>
					streaming_parse
						? parser(source.trim(), { streaming: true })
						: parser(source.trim()),
			}).pipe(
				Effect.catchTag("ConversationMarkdownParseFailure", () =>
					Effect.gen(function* () {
						return undefined;
					}),
				),
			);
		});

	/**
	 * One incremental parse of the currently revealed prefix. The parser keeps
	 * its previous output, so an append-only reveal re-parses only the message
	 * tail and every settled block keeps node identity — the renderer touches
	 * just the live edge instead of the whole transcript entry. A fence that
	 * closed this tick tokenizes here, between two cheap tail parses, instead
	 * of queueing for the settle frame. A parse failure keeps the previous
	 * tree on screen; the next reveal retries.
	 */
	const RenderRevealedMarkdown = Effect.gen(function* () {
		const source = revealed_text;
		if (parsed_text === source && parsed_parser_generation === streaming_parser_generation)
			return;
		const parser = streaming_parser;
		const generation = streaming_parser_generation;
		let tree = yield* ParseConversationMarkdown(parser, source, true);
		if (tree === undefined) return;
		if (HasPendingConversationFenceTokenization()) {
			yield* TokenizePendingConversationFences;
			const highlighted = yield* ParseConversationMarkdown(parser, source, true);
			if (highlighted !== undefined) tree = highlighted;
		}
		parsed_text = source;
		parsed_parser_generation = generation;
		rendered_tree = tree;
	});

	const RevealStreamingWords = Effect.gen(function* () {
		let current_text = revealed_text;
		let target = yield* Queue.take(streaming_word_targets);

		while (true) {
			const latest = yield* Queue.poll(streaming_word_targets);
			if (Option.isSome(latest)) target = latest.value;
			latest_streaming_target = target;
			if (target.streaming) presentation_settled = false;

			if (
				!should_animate_streaming_target(presentation_settled, target) ||
				!is_append_only_streaming_target(current_text, target) ||
				reduced_motion
			) {
				current_text = target.text;
				pending_animation_generation = undefined;
				revealed_text = target.text;
				presentation_settled = !target.streaming;
				yield* RenderRevealedMarkdown;
				target = yield* Queue.take(streaming_word_targets);
				continue;
			}

			const backlog = count_pending_streaming_words(current_text, target);
			const pacing = get_streaming_word_pacing(backlog);
			const next_text = reveal_streaming_words(current_text, target, pacing.words);
			if (next_text === current_text) {
				/** No new words, but a parser upgrade may still owe a re-render. */
				if (target.streaming || !presentation_settled) yield* RenderRevealedMarkdown;
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

			/**
			 * Commit the words, then pace. Racing the delay against the next
			 * transport target instead threw the computed words away every time a
			 * delta beat the tier delay — which, mid-stream, is nearly always — so
			 * the reveal only advanced once the provider paused. Corrections are
			 * still honoured at the top of the next tick, one stagger beat later.
			 */
			current_text = next_text;
			pending_animation_generation = ++animation_generation;
			revealed_text = next_text;
			yield* RenderRevealedMarkdown;
			yield* Effect.sleep(pacing.delay_ms);
		}
	});
	/** The component scope interrupts the reveal worker when its message unmounts. */
	yield* RevealStreamingWords.pipe(Effect.forkScoped);
	yield* Queue.offer(streaming_word_targets, { streaming, text });

	const presentation_streaming = $derived(
		streaming || revealed_text !== text || !presentation_settled,
	);

	/**
	 * Fence highlighting upgrades in place mid-stream: the moment an opened
	 * fence names a grammar, the highlighter chunk loads, the streaming parser
	 * is rebuilt with the fence plugin, and each fence is tokenised exactly
	 * once as it closes — instead of queueing every block for the settle frame.
	 */
	const streaming_highlight_requests = yield* Queue.unbounded<string>();
	let loaded_language_signature = "";
	const UpgradeStreamingHighlighting = Effect.gen(function* () {
		while (true) {
			const source = yield* Queue.take(streaming_highlight_requests);
			if (source.length === 0 || !has_conversation_fences(source)) continue;
			const languages = RequestedConversationFenceLanguages(source);
			const signature = languages.join(",");
			if (languages.length === 0 || signature === loaded_language_signature) continue;
			const plugin = yield* LoadConversationStreamingHighlightPlugin(
				source,
				() => open_fence_body.current,
			);
			if (plugin === undefined) continue;
			loaded_language_signature = signature;
			streaming_highlight_plugin = plugin;
			streaming_parser = MakeStreamingParser();
			streaming_parser_generation += 1;
			const wake = latest_streaming_target;
			if (wake !== undefined) yield* Queue.offer(streaming_word_targets, wake);
		}
	});
	/** The component scope interrupts the upgrade worker when its message unmounts. */
	yield* UpgradeStreamingHighlighting.pipe(Effect.forkScoped);
	yield* Queue.offer(streaming_highlight_requests, presentation_streaming ? revealed_text : "");

	/**
	 * The settled render owns the final tree: rich math and Mermaid nodes join
	 * at turn completion. When every requested grammar is already resident the
	 * message parses exactly once; otherwise it paints immediately without
	 * highlights and upgrades in place when the highlighter chunk arrives. The
	 * requested source stays empty until the reveal is genuinely settled.
	 */
	const RenderSettledTree = (source: string, plugins: ReadonlyArray<ComarkPlugin>) =>
		Effect.gen(function* () {
			const parser = createParse({
				...conversation_parse_options,
				plugins: [...plugins],
			});
			let tree = yield* ParseConversationMarkdown(parser, source, false);
			if (tree !== undefined && HasPendingConversationFenceTokenization()) {
				yield* TokenizePendingConversationFences;
				const highlighted = yield* ParseConversationMarkdown(parser, source, false);
				if (highlighted !== undefined) tree = highlighted;
			}
			if (tree === undefined) return;
			/** A newer streaming phase owns the render once the text moves on. */
			if (source !== revealed_text) return;
			parsed_text = undefined;
			rendered_tree = tree;
		});
	type SettledRenderRequest = {
		readonly generation: number;
		readonly source: string;
	};
	const settled_render_sources = yield* Queue.unbounded<SettledRenderRequest>();
	const RenderSettledMarkdown = Effect.gen(function* () {
		while (true) {
			const request = yield* Queue.take(settled_render_sources);
			const { source } = request;
			if (source.length === 0) continue;
			const resident = TryResidentConversationSettledMarkdownPlugins(source);
			if (resident === undefined && rendered_tree === undefined) {
				/** First paint must not wait for the highlighter chunk. */
				yield* RenderSettledTree(source, conversation_rich_markdown_plugins);
			}
			const plugins = resident ?? (yield* LoadConversationSettledMarkdownPlugins(source));
			/** Fences streamed mid-run are already cached; this covers the rest. */
			yield* WarmConversationFenceTokens(source);
			yield* RenderSettledTree(source, plugins);
			if (
				request.generation === settled_transform_generation &&
				source === revealed_text &&
				!presentation_streaming
			)
				settled_transform_pending = false;
		}
	});
	/** The component scope interrupts the settled worker when its message unmounts. */
	yield* RenderSettledMarkdown.pipe(Effect.forkScoped);
	const QueueSettledRender = (currently_streaming: boolean, source: string) =>
		Effect.gen(function* () {
			const settled_source = currently_streaming ? "" : source;
			const generation = ++settled_transform_generation;
			settled_transform_pending = settled_source.trim().length > 0;
			yield* Queue.offer(settled_render_sources, {
				generation,
				source: settled_source,
			});
		});
	yield* QueueSettledRender(presentation_streaming, revealed_text);

	const components = {
		ProseA: Anchor,
		ProseImg: Image,
		ProsePre: CodeSnippet,
		ProseMath: MathExpression,
		ProseMermaid: MermaidDiagram,
		ProseStreamWord: StreamWord,
	};
</script>

<!--
	The contents wrapper has no layout box. Its pending marker lets thread
	navigation distinguish "text has painted" from "the settled transform has
	finished" without coupling the route to every Markdown renderer.
-->
<div
	class="contents"
	data-thread-content-transforming={settled_transform_pending ? "true" : undefined}
>
	<!-- Rich nodes settle once at turn completion; partial math and Mermaid remain literal/code. -->
	{#if rendered_tree !== undefined}
		<ComarkRenderer
			class={`prose conversation-markdown ${presentation_streaming ? "conversation-markdown-streaming" : ""}`}
			tree={visible_tree}
			{components}
			streaming={presentation_streaming}
		/>
	{:else if revealed_text.trim().length > 0}
		<!--
			The transcript switches from its work summary to this message before the
			async Markdown parser necessarily finishes its first tree. Paint the exact
			text during that handoff; otherwise both surfaces are absent for one frame.
		-->
		<div
			class={`comark-content prose conversation-markdown ${presentation_streaming ? "conversation-markdown-streaming" : ""}`}
		>
			<p class="whitespace-pre-wrap">{revealed_text}</p>
		</div>
	{/if}
</div>
