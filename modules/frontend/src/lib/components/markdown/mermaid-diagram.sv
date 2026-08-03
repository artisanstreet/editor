<script lang="ts">
	import MermaidRenderer from "./mermaid-renderer.sv";
	let { content }: { content: string } = $props();
</script>

<svelte:boundary>
	<MermaidRenderer {content} />

	{#snippet pending()}
		<div class="docs-mermaid-diagram not-prose" data-render-status="loading" aria-busy="true">
			<pre class="docs-mermaid-source"><code>{content}</code></pre>
		</div>
	{/snippet}

	{#snippet failed()}
		<div class="docs-mermaid-diagram not-prose" data-render-status="invalid">
			<div class="docs-mermaid-error" role="note">
				<span>Unable to render this Mermaid diagram.</span>
				<pre><code>{content}</code></pre>
			</div>
		</div>
	{/snippet}
</svelte:boundary>
