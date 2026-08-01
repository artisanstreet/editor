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

<div class="composer-attachment-tray" aria-label="Attached images">
	<div class="composer-attachment-tray-content">
		{#each [...attachments.values()] as attachment (attachment.id)}
			<div class="composer-attachment-preview card">
				<button
					type="button"
					class="composer-attachment-preview-trigger"
					aria-label={`View ${attachment.name}`}
					onclick={yield* onview(attachment)}
				>
					<img src={attachment.preview_url} alt={attachment.name} />
				</button>
				<Button
					variant="secondary"
					size="icon-sm"
					class="composer-attachment-remove"
					aria-label={`Remove ${attachment.name}`}
					onclick={yield* onremove(attachment.id)}
				>
					<X class="size-3.5" />
				</Button>
			</div>
		{/each}
	</div>
</div>

<style>
	.composer-attachment-tray { display: grid; grid-template-rows: 0fr; opacity: 0; transition: grid-template-rows var(--composer-resize-dur) var(--composer-resize-ease), opacity 150ms ease-out; }
	:global(.thread-composer[data-has-attachments="true"]) .composer-attachment-tray { grid-template-rows: 1fr; opacity: 1; }
	.composer-attachment-tray-content { min-height: 0; display: flex; gap: .5rem; overflow: hidden; padding: 0; transition: padding var(--composer-resize-dur) var(--composer-resize-ease); }
	:global(.thread-composer[data-has-attachments="true"]) .composer-attachment-tray-content { padding: .25rem .25rem .5rem; }
	.composer-attachment-preview { position: relative; width: 4.5rem; height: 4.5rem; flex: none; overflow: hidden; border-radius: .9rem; opacity: 0; transform: translateY(8px) scale(.96); transition: opacity 180ms var(--composer-resize-ease), transform var(--composer-resize-dur) var(--composer-resize-ease); }
	:global(.thread-composer[data-has-attachments="true"]) .composer-attachment-preview { opacity: 1; transform: translateY(0) scale(1); }
	.composer-attachment-preview-trigger { display: block; width: 100%; height: 100%; padding: 0; border: 0; background: transparent; cursor: pointer; }
	.composer-attachment-preview-trigger:focus-visible { outline: 2px solid var(--ring); outline-offset: -2px; }
	.composer-attachment-preview img { width: 100%; height: 100%; object-fit: cover; }
	:global(.composer-attachment-remove) { position: absolute; top: .2rem; right: .2rem; min-width: 1.35rem; width: 1.35rem; height: 1.35rem; border-radius: 999px; background: rgb(255 255 255 / .92); color: #18181b; }

	@media (prefers-reduced-motion: reduce) {
		.composer-attachment-tray,
		.composer-attachment-tray-content,
		.composer-attachment-preview {
			transition: none !important;
			will-change: auto;
		}
	}
</style>
