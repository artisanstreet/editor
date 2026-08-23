<script lang="ts" effect>
	import { lookup_artisan_error } from "@artisan/catalog";
	import type { ConversationErrorRef } from "@artisan/protocol";
	import CircleX from "@tabler/icons-svelte/icons/circle-x";
	import Copy from "@tabler/icons-svelte/icons/copy";
	import { Effect } from "effect";
	import { FormatLocalDateTime } from "$lib/appearance/display-format";
	import { time_format } from "$lib/appearance-config";
	import { RunBrowserDom } from "$lib/browser/dom";

	let {
		error,
		detail,
	}: {
		error: ConversationErrorRef;
		/**
		 * The diagnostic's own summary line — the provider's account of the
		 * failure. The card's explanation comes from the error catalog; this is
		 * the evidence line beneath it, and the ref's captured detail wins over
		 * the projected summary when both exist.
		 */
		detail?: string;
	} = $props();

	/**
	 * Identity resolves through the catalog, never from the wire: an unknown
	 * code — a newer backend than this renderer — degrades to the unknown
	 * definition while still displaying the code itself, which is the durable
	 * artifact a docs page will be keyed on.
	 */
	const definition = $derived(lookup_artisan_error(error.code));
	const detail_text = $derived(error.detail ?? detail);
	const resets_label = $derived(
		error.resets_at === undefined
			? undefined
			: FormatLocalDateTime(error.resets_at, $time_format, {
					dateStyle: "medium",
					timeStyle: "short",
				}),
	);

	const CopyCode = () =>
		Effect.gen(function* () {
			yield* RunBrowserDom(() => void navigator.clipboard.writeText(error.code));
		});
</script>

<div
	class="flex w-full min-w-0 flex-col gap-1.5 rounded-xl border border-destructive/25 bg-destructive/5 px-3.5 py-3"
>
	<div class="flex min-w-0 items-center gap-2">
		<CircleX class="size-4 shrink-0 text-destructive" aria-hidden="true" />
		<span class="min-w-0 flex-1 truncate text-sm font-medium text-destructive">
			{definition.title}
		</span>
		<button
			type="button"
			class="flex shrink-0 cursor-pointer items-center gap-1 font-mono text-xs text-muted-foreground transition-colors duration-150 hover:text-foreground motion-reduce:transition-none"
			aria-label={`Copy error code ${error.code}`}
			onclick={yield* CopyCode()}
		>
			{error.code}
			<Copy class="size-3" aria-hidden="true" />
		</button>
	</div>

	<p class="text-sm text-muted-foreground">
		{definition.summary}
		{#if resets_label !== undefined}
			Resets {resets_label}.
		{/if}
	</p>

	{#if detail_text !== undefined}
		<span class="min-w-0 font-mono text-xs break-words text-muted-foreground/70">
			{detail_text}
		</span>
	{/if}
</div>
