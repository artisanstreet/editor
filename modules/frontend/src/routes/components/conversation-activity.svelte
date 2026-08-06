<script lang="ts">
	import {
		GetConversationActivityPresentation,
		type ConversationItem,
	} from "@artisan/protocol";
	import type { Snippet } from "svelte";

	let {
		item,
		trailing,
	}: {
		item: Extract<ConversationItem, { type: "activity" }>;
		trailing?: Snippet;
	} = $props();
	const presentation = $derived(GetConversationActivityPresentation(item));
</script>

<div class="flex items-baseline gap-3 text-sm">
	<span class="shrink-0 text-foreground">{presentation.label}</span>
	{#if item.detail !== undefined}
		<span class="min-w-0 truncate text-muted-foreground">{item.detail}</span>
	{/if}
	{#if trailing !== undefined}{@render trailing()}{/if}
</div>
