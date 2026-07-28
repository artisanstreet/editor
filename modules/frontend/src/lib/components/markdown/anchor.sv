<script lang="ts">
	import type { Snippet } from "svelte";

	let {
		children,
		href,
	}: {
		children?: Snippet;
		href?: string;
	} = $props();

	/**
	 * Assistant links are untrusted, so only well-known protocols become live
	 * anchors; anything else renders as plain text. markdown-it already vets
	 * link protocols during parsing, so this guard is defense in depth.
	 */
	const safe_href = $derived.by(() => {
		if (href === undefined) return undefined;
		try {
			const protocol = new URL(href, "https://conversation.invalid").protocol;
			return protocol === "https:" || protocol === "http:" || protocol === "mailto:"
				? href
				: undefined;
		} catch {
			return undefined;
		}
	});
</script>

{#if safe_href === undefined}
	{@render children?.()}
{:else}
	<a href={safe_href} target="_blank" rel="noopener noreferrer">{@render children?.()}</a>
{/if}
