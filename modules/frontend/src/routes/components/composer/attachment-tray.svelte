<script lang="ts" effect>
	import X from "@tabler/icons-svelte/icons/x";
	import type { Effect } from "effect";
	import { Button } from "$lib/components/ui/button";
	import type { ComposerImageAttachment } from "$lib/composer/image-attachments";

	let {
		attachments,
		onremove,
		onview,
	}: {
		attachments: ReadonlyMap<string, ComposerImageAttachment>;
		onremove: (attachment_id: string) => Effect.Effect<void>;
		onview: (attachment: ComposerImageAttachment) => Effect.Effect<void>;
	} = $props();
</script>

<!--
	The tray opens because the composer above it has attachments, so every state
	here is read off that ancestor's `group/composer` rather than mirrored into a
	prop. `grid-template-rows` 0fr → 1fr gives the height a clean tween with no
	measurement, and the content clips its own overflow.
-->
<div
	class="grid grid-rows-[0fr] opacity-0 [transition-property:grid-template-rows,opacity] duration-(--composer-resize-dur) ease-(--composer-resize-ease) group-data-[has-attachments=true]/composer:grid-rows-[1fr] group-data-[has-attachments=true]/composer:opacity-100"
	aria-label="Attached images"
>
	<div
		class="flex min-h-0 gap-2 overflow-hidden p-0 [transition-property:padding] duration-(--composer-resize-dur) ease-(--composer-resize-ease) group-data-[has-attachments=true]/composer:px-1 group-data-[has-attachments=true]/composer:pt-1 group-data-[has-attachments=true]/composer:pb-2"
	>
		{#each [...attachments.values()] as attachment (attachment.id)}
			<div class="card relative size-18 flex-none translate-y-2 scale-96 overflow-hidden rounded-xl opacity-0 [transition-property:opacity,transform] duration-(--composer-resize-dur) ease-(--composer-resize-ease) group-data-[has-attachments=true]/composer:translate-y-0 group-data-[has-attachments=true]/composer:scale-100 group-data-[has-attachments=true]/composer:opacity-100">
				<button
					type="button"
					class="block size-full cursor-pointer border-0 bg-transparent p-0 focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring"
					aria-label={`View ${attachment.name}`}
					onclick={yield* onview(attachment)}
				>
					<img src={attachment.preview_url} alt={attachment.name} class="size-full object-cover" />
				</button>
				<Button
					variant="secondary"
					size="icon-sm"
					class="absolute top-[0.2rem] right-[0.2rem] size-5.5 min-w-5.5 rounded-full bg-surface-0/92 text-surface-900"
					aria-label={`Remove ${attachment.name}`}
					onclick={yield* onremove(attachment.id)}
				>
					<X class="size-3.5" />
				</Button>
			</div>
		{/each}
	</div>
</div>
