<script lang="ts">
	import { Comark } from "@comark/svelte";
	import Anchor from "./anchor.sv";
	import CodeSnippet from "./code-snippet.sv";
	import Image from "./image.sv";
	import {
		conversation_markdown_plugins,
		conversation_streaming_markdown_plugins,
	} from "./highlighting";
	import MathExpression from "./math-expression.sv";
	import MermaidDiagram from "./mermaid-diagram.sv";
	import { conversation_parse_options } from "./parsing";

	let { streaming = false, text }: { streaming?: boolean; text: string } = $props();
	const active_plugins = $derived(
		streaming ? conversation_streaming_markdown_plugins : conversation_markdown_plugins,
	);

	const components = {
		ProseA: Anchor,
		ProseImg: Image,
		ProsePre: CodeSnippet,
		ProseMath: MathExpression,
		ProseMermaid: MermaidDiagram,
	};
</script>

<!-- Rich nodes settle once at turn completion; partial math and Mermaid remain literal/code. -->
<Comark
	class="prose conversation-markdown"
	markdown={text}
	options={conversation_parse_options}
	plugins={active_plugins}
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
