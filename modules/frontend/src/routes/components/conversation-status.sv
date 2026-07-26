<script lang="ts">
	import { Alert, AlertDescription, AlertTitle } from "$lib/components/ui/alert";
	import { Badge } from "$lib/components/ui/badge";
	import type { ConversationItem } from "@artisan/protocol";
	import type { Snippet } from "svelte";

	let {
		item,
		trailing,
	}: {
		item: Extract<ConversationItem, { type: "error" | "compaction" | "native_event" }>;
		trailing?: Snippet;
	} = $props();
</script>

{#if item.type === "error"}
	<Alert variant="destructive" class="max-w-2xl"><AlertTitle>Error · retry available</AlertTitle><AlertDescription>{item.message}</AlertDescription></Alert>
{:else}
	<div class="flex items-center gap-2 text-sm text-muted-foreground">
		<Badge variant="outline">{item.type === "compaction" ? "Compacted" : "Native"}</Badge>
		<span>{item.summary}</span>
		{#if trailing !== undefined}{@render trailing()}{/if}
	</div>
{/if}
