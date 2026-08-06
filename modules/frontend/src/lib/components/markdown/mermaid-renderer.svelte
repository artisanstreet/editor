<script lang="ts" effect>
	import { Data, Effect } from "effect";

	class MermaidRendererLoadFailure extends Data.TaggedError("MermaidRendererLoadFailure")<{
		readonly cause: unknown;
	}> {}

	let { content }: { content: string } = $props();
	const { render_conversation_mermaid } = yield* Effect.tryPromise({
		catch: (cause) => new MermaidRendererLoadFailure({ cause }),
		try: () => import("./mermaid-rendering"),
	});
	const rendered = $derived(render_conversation_mermaid(content));
</script>

<div
	class="docs-mermaid-diagram not-prose"
	data-render-status={rendered.status}
	role={rendered.status === "rendered" ? "img" : undefined}
	aria-label={rendered.status === "rendered" ? "Mermaid diagram" : undefined}
>
	{#if rendered.status === "rendered"}
		<!-- The adapter accepts only passive, structurally validated SVG. -->
		{@html rendered.html}
	{:else}
		<div class="docs-mermaid-error" role="note">
			<span>Unable to render this Mermaid diagram.</span>
			<pre><code>{content}</code></pre>
		</div>
	{/if}
</div>
