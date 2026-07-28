<script lang="ts">
	import { Comark } from "@comark/svelte";
	import Anchor from "./anchor.sv";
	import Image from "./image.sv";
	import { conversation_parse_options } from "./parsing";

	let { streaming = false, text }: { streaming?: boolean; text: string } = $props();

	const components = { ProseA: Anchor, ProseImg: Image };
</script>

<Comark
	class="prose conversation-markdown"
	markdown={text}
	options={conversation_parse_options}
	{components}
	{streaming}
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
