<script lang="ts" effect>
	import { BannerService } from "$lib/banner/service";
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

	const banner = yield* BannerService;
	if (item.type === "error") {
		yield* banner.error("Thread error", { description: item.message });
	}
</script>

{#if item.type !== "error"}
	<div class="flex items-center gap-2 text-sm text-muted-foreground">
		<Badge variant="outline">{item.type === "compaction" ? "Compacted" : "Native"}</Badge>
		<span>{item.summary}</span>
		{#if trailing !== undefined}{@render trailing()}{/if}
	</div>
{/if}
