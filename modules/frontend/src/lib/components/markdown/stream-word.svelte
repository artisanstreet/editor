<script lang="ts">
	import { untrack, type Snippet } from "svelte";

	let {
		children,
		incoming = false,
	}: {
		children?: Snippet;
		incoming?: boolean;
	} = $props();

	/**
	 * Latch the mount state: once another word arrives the AST moves its
	 * `incoming` marker forward, but this word must finish its own entrance.
	 */
	const reveal_on_mount = untrack(() => incoming);
	let revealing = $state(reveal_on_mount);
</script>

<span
	class="docs-stream-word"
	class:docs-stream-word-incoming={revealing}
	onanimationend={() => (revealing = false)}
>
	{@render children?.()}
</span>
