<script lang="ts">
	import { Badge } from "$lib/components/ui/badge";
	import { Card, CardContent } from "$lib/components/ui/card";
	import type { ConversationItem } from "@artisan/protocol";
	import { FormatPathSeparators } from "$lib/appearance/display-format";
	import { path_separator } from "$lib/appearance-config";

	let { item }: { item: Extract<ConversationItem, { type: "change_set" | "file_change" }> } = $props();
</script>

<Card size="sm" class="max-w-(--prose-body-width) py-3">
	<CardContent class="flex items-center gap-2">
		<Badge variant="outline">{item.type === "change_set" ? "Changes" : "File"}</Badge>
		<span class="min-w-0 truncate text-sm"
			>{item.type === "change_set"
				? item.summary
				: FormatPathSeparators(item.path, $path_separator)}</span
		>
	</CardContent>
</Card>
