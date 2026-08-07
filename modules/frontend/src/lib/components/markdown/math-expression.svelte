<script lang="ts">
	import MathRenderer from "./math-renderer.svelte";

	let {
		content,
		class: class_name = "",
	}: {
		content: string;
		class?: string;
	} = $props();
	const is_inline = $derived(class_name.split(" ").includes("inline"));

</script>

<svelte:boundary>
	<MathRenderer {content} class={class_name} />

	{#snippet pending()}
		{#if is_inline}
			<code class="docs-math-fallback" aria-busy="true">{content}</code>
		{:else}
			<pre class="docs-math-fallback not-prose" aria-busy="true"><code>{content}</code></pre>
		{/if}
	{/snippet}

	{#snippet failed()}
		{#if is_inline}
			<code class="docs-math-fallback" role="note">{content}</code>
		{:else}
			<pre class="docs-math-fallback not-prose" role="note"><code>{content}</code></pre>
		{/if}
	{/snippet}
</svelte:boundary>
