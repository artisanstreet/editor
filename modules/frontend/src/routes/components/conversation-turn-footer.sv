<script lang="ts" effect>
	import Copy from "@tabler/icons-svelte/icons/copy";
	import { Button } from "$lib/components/ui/button";
	import { format_relative_age } from "$lib/conversation/relative-time";
	import { Clock, Effect } from "effect";

	let {
		settled_at,
		text,
	}: {
		settled_at: string;
		text: string;
	} = $props();

	let now = $state(yield* Clock.currentTimeMillis);
	const age = $derived(format_relative_age(now, settled_at));

	yield* Effect.forever(
		Effect.gen(function* () {
			yield* Effect.sleep("1 second");
			now = yield* Clock.currentTimeMillis;
		}),
	);

	const copy_response = () => {
		void navigator.clipboard.writeText(text);
	};
</script>

<footer
	class="pointer-events-none absolute top-[calc(100%+0.25rem)] left-0 z-10 flex items-center gap-1 text-sm text-muted-foreground opacity-0 transition-opacity duration-(--duration-quick) ease-out group-hover/turn:pointer-events-auto group-hover/turn:opacity-100 group-focus-within/turn:pointer-events-auto group-focus-within/turn:opacity-100 motion-reduce:transition-none"
	aria-label="Turn actions"
>
	<Button
		variant="ghost"
		size="icon-xs"
		class="text-muted-foreground"
		aria-label="Copy response"
		title="Copy response"
		onclick={copy_response}
	>
		<Copy class="size-4" />
	</Button>
	<time datetime={settled_at}>{age}</time>
</footer>
