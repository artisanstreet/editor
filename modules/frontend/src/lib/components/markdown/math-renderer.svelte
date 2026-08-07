<script lang="ts" effect>
	import { Data, Effect } from "effect";

	class MathRendererLoadFailure extends Data.TaggedError("MathRendererLoadFailure")<{
		readonly cause: unknown;
	}> {}

	let {
		content,
		class: class_name = "",
	}: {
		content: string;
		class?: string;
	} = $props();
	const is_inline = $derived(class_name.split(" ").includes("inline"));
	yield* Effect.tryPromise({
		catch: (cause) => new MathRendererLoadFailure({ cause }),
		try: () => import("katex/dist/katex.min.css"),
	});
	const { render_conversation_math } = yield* Effect.tryPromise({
		catch: (cause) => new MathRendererLoadFailure({ cause }),
		try: () => import("./math-rendering"),
	});
	const rendered = $derived(render_conversation_math(content, !is_inline));
</script>

{#if rendered.status === "rendered"}
	{#if is_inline}
		<!-- KaTeX returns escaped, trust-disabled markup. -->
		<span class="docs-math-inline">{@html rendered.html}</span>
	{:else}
		<!-- KaTeX returns escaped, trust-disabled markup. -->
		<div class="docs-math-block not-prose">{@html rendered.html}</div>
	{/if}
{:else if is_inline}
	<code class="docs-math-fallback">{content}</code>
{:else}
	<pre class="docs-math-fallback not-prose"><code>{content}</code></pre>
{/if}
