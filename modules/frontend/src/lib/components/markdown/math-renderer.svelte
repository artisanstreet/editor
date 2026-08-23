<script lang="ts" effect>
	import { Effect, Stream } from "effect";
	import {
		MathRendererController,
		type MathRendererState,
	} from "./math-renderer-controller";

	let {
		content,
		class: class_name = "",
	}: {
		content: string;
		class?: string;
	} = $props();
	const is_inline = $derived(class_name.split(" ").includes("inline"));
	const renderer_controller = yield* MathRendererController;
	let renderer_state = $state.raw<MathRendererState>(yield* renderer_controller.Current);
	const ApplyRendererState = (next: MathRendererState) =>
		Effect.gen(function* () {
			renderer_state = next;
		});
	yield* renderer_controller.Changes.pipe(Stream.runForEach(ApplyRendererState), Effect.forkScoped);
	if (renderer_state._tag === "Loading") {
		yield* renderer_controller.Refresh.pipe(Effect.forkScoped);
	}
	const rendered = $derived(
		renderer_state._tag === "Ready"
			? renderer_state.render(content, !is_inline)
			: { status: "invalid" } as const,
	);
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
	<code
		class="docs-math-fallback"
		aria-busy={renderer_state._tag === "Loading" ? "true" : undefined}
	>{content}</code>
{:else}
	<pre
		class="docs-math-fallback not-prose"
		aria-busy={renderer_state._tag === "Loading" ? "true" : undefined}
	><code>{content}</code></pre>
{/if}
