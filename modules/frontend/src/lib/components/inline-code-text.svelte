<script lang="ts">
	import { conversation_summary_fragments } from "$lib/conversation/trace";

	/**
	 * One line of model prose with its backticked runs set in the code face the
	 * model meant them to have, instead of prose wearing two stray marks. The
	 * splitting grammar is the thinking line's: a span whose closing backtick
	 * has not arrived yet still reads as code from its opening mark, so a
	 * streaming sentence never shows a bare backtick and then reflows.
	 *
	 * Renders inline fragments only — no wrapper element — so it inherits
	 * whatever size, tone, clamping, and shimmer its parent has, and the code
	 * face travels with the sentence through all of them.
	 */
	let { text }: { text: string } = $props();
</script>

{#each conversation_summary_fragments(text) as fragment, index (index)}
	{#if fragment.code}<code
			class="font-mono text-[0.9em]">{fragment.text}</code>{:else}{fragment.text}{/if}
{/each}
