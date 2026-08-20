<script lang="ts">
	import { conversation_summary_fragments } from "$lib/conversation/trace";

	/**
	 * One line of model prose with its inline marks honoured — backticked runs
	 * in the code face, matched `**`/`__`/`*`/`_`/`~~` pairs in their weight
	 * and style — instead of prose wearing stray punctuation. The splitting
	 * grammar is the thinking line's: a span whose closing backtick has not
	 * arrived yet still reads as code from its opening mark, so a streaming
	 * sentence never shows a bare backtick and then reflows.
	 *
	 * Renders inline fragments only — no wrapper element — so it inherits
	 * whatever size, tone, clamping, and shimmer its parent has. Emphasis
	 * deliberately changes weight and posture but never colour: a bold run in
	 * a muted preview stays muted, because the line's tone belongs to the
	 * surface, not to the sentence's own punctuation.
	 */
	let { text }: { text: string } = $props();
</script>

{#each conversation_summary_fragments(text) as fragment, index (index)}
	{#if fragment.code}<code
			class="font-mono text-[0.9em]">{fragment.text}</code>{:else if fragment.strong || fragment.em || fragment.strike}<span
			class={`text-inherit ${fragment.strong ? "font-semibold" : ""} ${fragment.em ? "italic" : ""} ${fragment.strike ? "line-through" : ""}`}>{fragment.text}</span>{:else}{fragment.text}{/if}
{/each}
