<script lang="ts">
	import { ShimmerText } from "$lib/components/ui/shimmer-text";
	import { user_message_style_config } from "$lib/conversation-style-config";
	import type { ConversationItem, ImageAttachmentReference } from "@artisan/protocol";
	import type { Snippet } from "svelte";
	import ImageViewer from "./image-viewer.sv";

	type ResolvedImageAttachment = {
		readonly attachment: ImageAttachmentReference;
		readonly source: string;
	};

	let {
		image_sources,
		item,
		onimagevisibilitychange,
		trailing,
	}: {
		image_sources?: ReadonlyMap<string, string>;
		item: Extract<
			ConversationItem,
			{ type: "user_message" | "assistant_message" | "reasoning_summary" }
		>;
		onimagevisibilitychange?: (
			attachments: ReadonlyArray<ImageAttachmentReference>,
			visible: boolean,
		) => void;
		trailing?: Snippet;
	} = $props();

	let image_viewer_open = $state(false);
	let viewed_image = $state<ResolvedImageAttachment | undefined>();
	let image_group_visible = $state(false);
	const resolved_images = $derived.by((): ReadonlyArray<ResolvedImageAttachment> => {
		if (item.type !== "user_message") return [];
		return (item.attachments ?? []).flatMap((attachment) => {
			const source = image_sources?.get(attachment.id);
			return source === undefined ? [] : [{ attachment, source }];
		});
	});

	const view_image = (image: ResolvedImageAttachment) => {
		viewed_image = image;
		image_viewer_open = true;
	};

	const observe_image_visibility = (node: HTMLElement) => {
		if (item.type !== "user_message" || item.attachments === undefined) return;
		const attachments = item.attachments;
		const update_visibility = (visible: boolean) => {
			image_group_visible = visible;
			if (visible || !image_viewer_open) onimagevisibilitychange?.(attachments, visible);
		};
		if (!("IntersectionObserver" in globalThis)) {
			let frame = 0;
			const measure = () => {
				cancelAnimationFrame(frame);
				frame = requestAnimationFrame(() => {
					const bounds = node.getBoundingClientRect();
					update_visibility(
						bounds.bottom >= -160 && bounds.top <= globalThis.innerHeight + 160,
					);
				});
			};
			globalThis.addEventListener("resize", measure);
			globalThis.addEventListener("scroll", measure, true);
			measure();
			return {
				destroy: () => {
					cancelAnimationFrame(frame);
					globalThis.removeEventListener("resize", measure);
					globalThis.removeEventListener("scroll", measure, true);
					onimagevisibilitychange?.(attachments, false);
				},
			};
		}
		const observer = new IntersectionObserver(
			(entries) => {
				const visible = entries.some((entry) => entry.isIntersecting);
				update_visibility(visible);
			},
			{ rootMargin: "160px 0px" },
		);
		observer.observe(node);
		return {
			destroy: () => {
				observer.disconnect();
				onimagevisibilitychange?.(attachments, false);
			},
		};
	};
</script>

{#if item.type === "user_message"}
	<article
		class="ml-auto flex max-w-xl flex-col items-end gap-2"
		aria-label="Your message"
		data-conversation-item-id={item.id}
	>
		{#if (item.attachments?.length ?? 0) > 0}
			<div
				use:observe_image_visibility
				class="flex max-w-full flex-wrap justify-end gap-2"
				aria-label="Attached images"
			>
				{#each item.attachments ?? [] as attachment (attachment.id)}
					{@const image = resolved_images.find((candidate) => candidate.attachment.id === attachment.id)}
					{#if image === undefined}
						<div class="card conversation-image-thumbnail animate-pulse bg-muted/60" aria-hidden="true"></div>
					{:else}
						<button
							type="button"
							class="card conversation-image-thumbnail"
							aria-label={`View ${image.attachment.name}`}
							onclick={() => view_image(image)}
						>
							<img src={image.source} alt={image.attachment.name} />
						</button>
					{/if}
				{/each}
			</div>
		{/if}
		<div
			class="user-message max-w-full rounded-2xl px-4 py-3"
			class:card={$user_message_style_config.use_card}
			style:--user-message-from={`var(--${$user_message_style_config.from})`}
			style:--user-message-to={`var(--${$user_message_style_config.to})`}
		>
			{#if item.text.length > 0}
				<p class="whitespace-pre-wrap text-base leading-7 text-foreground">{item.text}</p>
			{/if}
			{#if trailing !== undefined}{@render trailing()}{/if}
		</div>
	</article>
{:else}
	<article class="max-w-2xl" aria-label={item.type === "reasoning_summary" ? "Reasoning summary" : "Assistant message"}>
		{#if item.type === "reasoning_summary"}
			<ShimmerText
				class="whitespace-pre-wrap text-base leading-7 text-muted-foreground"
				delay={0}
				duration={2}
			>
				{item.text}
			</ShimmerText>
		{:else}
			<p class="whitespace-pre-wrap text-base leading-7 text-foreground">{item.text}</p>
			{#if trailing !== undefined}{@render trailing()}{/if}
		{/if}
	</article>
{/if}

<ImageViewer
	bind:open={image_viewer_open}
	source={viewed_image?.source}
	name={viewed_image?.attachment.name}
	onclose={() => {
		if (!image_group_visible && item.type === "user_message") {
			onimagevisibilitychange?.(item.attachments ?? [], false);
		}
		viewed_image = undefined;
	}}
/>

<style>
	.user-message {
		background-image: linear-gradient(
			to top,
			var(--user-message-from),
			var(--user-message-to)
		);
	}

	.conversation-image-thumbnail {
		width: 6rem;
		height: 6rem;
		padding: 0;
		overflow: hidden;
		border-radius: .75rem;
		background: transparent;
		cursor: pointer;
	}

	.conversation-image-thumbnail:focus-visible {
		outline: 2px solid var(--ring);
		outline-offset: 2px;
	}

	.conversation-image-thumbnail img {
		width: 100%;
		height: 100%;
		object-fit: cover;
	}
</style>
