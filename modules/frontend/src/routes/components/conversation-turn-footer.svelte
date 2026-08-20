<script lang="ts" effect>
	import Copy from "@tabler/icons-svelte/icons/copy";
	import { WriteClipboardText } from "$lib/browser/clipboard";
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
	let copy_message = $state("");
	const age = $derived(format_relative_age(now, settled_at));

	/** Settled history has no reason to wake the renderer until its actions are shown. */
	const RefreshAge = Effect.gen(function* () {
		now = yield* Clock.currentTimeMillis;
	});

	const CopyResponse = Effect.gen(function* () {
		copy_message = "";
		yield* WriteClipboardText(text).pipe(
			Effect.catchTag("ClipboardWriteError", () =>
				Effect.gen(function* () {
					copy_message = "Couldn't copy response. Try again.";
				}),
			),
		);
	});

</script>

<footer
	class="pointer-events-none absolute top-[calc(100%+0.25rem)] left-0 z-10 flex items-center gap-1 text-sm text-muted-foreground opacity-0 transition-opacity duration-(--duration-quick) ease-out group-hover/turn:pointer-events-auto group-hover/turn:opacity-100 group-focus-within/turn:pointer-events-auto group-focus-within/turn:opacity-100 motion-reduce:transition-none"
	aria-label="Turn actions"
	onmouseenter={yield* RefreshAge}
	onfocusin={yield* RefreshAge}
>
	<Button
		variant="ghost"
		size="icon-xs"
		class="text-muted-foreground"
		aria-label="Copy response"
		title="Copy response"
		onclick={yield* CopyResponse}
	>
		<Copy class="size-4" />
	</Button>
	{#if copy_message.length > 0}
		<span class="text-destructive" role="status">{copy_message}</span>
	{/if}
	<time datetime={settled_at}>{age}</time>
</footer>
