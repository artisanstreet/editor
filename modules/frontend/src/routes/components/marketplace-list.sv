<script lang="ts" effect>
	import { Effect, Option } from "effect";
	import { IconBolt as Bolt, IconCloud as Cloud } from "@tabler/icons-svelte";

	import type { LiveWorkspaceSnapshot } from "$lib/live-workspace/store";
	import { Badge } from "$lib/components/ui/badge";
	import { Button } from "$lib/components/ui/button";
	import { Card, CardContent } from "$lib/components/ui/card";
	import { ScrollArea } from "$lib/components/ui/scroll-area";

	type MarketplaceScope =
		| { readonly kind: "global" }
		| { readonly kind: "workspace"; readonly workspace_id: string }
		| { readonly kind: "project"; readonly project_id: string };

	let {
		live_snapshot,
		query,
		show_routines,
		show_capabilities,
		SelectRoutine,
		SelectCapability,
	}: {
		live_snapshot: LiveWorkspaceSnapshot;
		query: string;
		show_routines: boolean;
		show_capabilities: boolean;
		SelectRoutine: (id: string, scope: MarketplaceScope) => Effect.Effect<void>;
		SelectCapability: (id: string, scope: MarketplaceScope) => Effect.Effect<void>;
	} = $props();

	const IncludesQuery = (value: string) => value.toLowerCase().includes(query.toLowerCase());
</script>

<ScrollArea class="h-full rounded-md border">
	<div class="grid gap-2 p-2">
		{#if show_routines && Option.isSome(live_snapshot.routines)}
			{#each live_snapshot.routines.value.routines.filter((item) => IncludesQuery(`${item.display_name} ${item.description}`)) as item (item.id)}
				<Card><CardContent class="flex items-center gap-3 p-3"><Bolt class="text-muted-foreground" size={17} aria-hidden="true" /><Button class="h-auto min-w-0 flex-1 justify-start px-0 text-left" variant="ghost" onclick={yield* SelectRoutine(item.id, item.scope)}><span class="min-w-0"><strong class="block truncate text-sm">{item.display_name}</strong><span class="text-muted-foreground block truncate text-xs">{item.description}</span></span></Button><Badge variant="secondary">{item.status}</Badge></CardContent></Card>
			{/each}
		{/if}
		{#if show_capabilities && Option.isSome(live_snapshot.capabilities)}
			{#each live_snapshot.capabilities.value.capabilities.filter((item) => IncludesQuery(item.display_name)) as item (item.id)}
				<Card><CardContent class="flex items-center gap-3 p-3"><Cloud class="text-muted-foreground" size={17} aria-hidden="true" /><Button class="h-auto min-w-0 flex-1 justify-start px-0 text-left" variant="ghost" onclick={yield* SelectCapability(item.id, item.scope)}><span class="min-w-0"><strong class="block truncate text-sm">{item.display_name}</strong><span class="text-muted-foreground block text-xs">{item.transport_kind} · {item.health.status}</span></span></Button><Badge variant="secondary">{item.lifecycle}</Badge></CardContent></Card>
			{/each}
		{/if}
		{#if (!show_routines || Option.isNone(live_snapshot.routines)) && (!show_capabilities || Option.isNone(live_snapshot.capabilities))}<p class="text-muted-foreground p-4 text-sm">No Marketplace records have loaded yet.</p>{/if}
	</div>
</ScrollArea>
