<script lang="ts" effect>
	import Pencil from "@tabler/icons-svelte/icons/pencil";
	import Trash from "@tabler/icons-svelte/icons/trash";
	import type { Effect } from "effect";
	import { Button } from "$lib/components/ui/button";

	let {
		editable,
		ondiscard,
		onedit,
		text,
	}: {
		/** Recalling needs a route that can reach Forge; without one the row only informs. */
		editable: boolean;
		ondiscard: Effect.Effect<void>;
		onedit: Effect.Effect<void>;
		text: string;
	} = $props();
</script>

<!--
	One queued steer, one line: the row must not grow the lip while a run is
	streaming underneath, so the text keeps to a single line-height and elides.
	The actions answer the queued state's one honest question — take the message
	back to edit it, or let it go — and they leave with the lip the moment the
	engine takes the steer up.
-->
<div
	class="flex items-center gap-3 bg-surface-125/90 py-2 pr-2 pl-5 text-base text-foreground/70 dark:bg-surface-850/90"
	role="status"
>
	<p class="min-w-0 flex-1 truncate">{text}</p>
	{#if editable}
		<div class="flex shrink-0 items-center gap-1">
			<Button
				variant="ghost"
				size="icon-sm"
				class="text-muted-foreground hover:text-foreground"
				aria-label="Edit queued message"
				title="Edit queued message"
				onclick={yield* onedit}
			>
				<Pencil class="size-4" aria-hidden="true" />
			</Button>
			<Button
				variant="ghost"
				size="icon-sm"
				class="text-muted-foreground hover:text-foreground"
				aria-label="Discard queued message"
				title="Discard queued message"
				onclick={yield* ondiscard}
			>
				<Trash class="size-4" aria-hidden="true" />
			</Button>
		</div>
	{/if}
</div>
