<script lang="ts" effect>
	import { Effect } from "effect";
	import MarkdownContent from "$lib/components/markdown/content.svelte";
	import { RunBrowserDom } from "$lib/browser/dom";
	import { MakeScopedAttachmentRunner } from "$lib/lifecycle/scoped-attachment-runner";
	import { ShimmerText } from "$lib/components/ui/shimmer-text";
	import { user_message_style_config } from "$lib/conversation-style-config";
	import type { ConversationItem, ImageAttachmentReference } from "@artisan/protocol";
	import type { Snippet } from "svelte";
	import ImageViewer from "./image-viewer.svelte";

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
		) => Effect.Effect<void>;
		trailing?: Snippet;
	} = $props();

	let image_viewer_open = $state(false);
	let viewed_image = $state<ResolvedImageAttachment | undefined>();
	let image_group_visible = $state(false);
	type VisibilityWork =
		| {
				readonly _tag: "Measure";
				readonly attachments: ReadonlyArray<ImageAttachmentReference>;
				readonly node: HTMLElement;
				readonly visibility_key: string;
		  }
		| { readonly _tag: "Observe"; readonly node: HTMLElement; readonly visibility_key: string }
		| {
				readonly _tag: "Visibility";
				readonly attachments: ReadonlyArray<ImageAttachmentReference>;
				readonly visible: boolean;
		  };
	const resolved_images = $derived.by((): ReadonlyArray<ResolvedImageAttachment> => {
		if (item.type !== "user_message") return [];
		return (item.attachments ?? []).flatMap((attachment) => {
			const source = image_sources?.get(attachment.id);
			return source === undefined ? [] : [{ attachment, source }];
		});
	});

	const ViewImage = (image: ResolvedImageAttachment) =>
		Effect.gen(function* () {
			viewed_image = image;
			image_viewer_open = true;
		});

	const ObserveImageVisibility = (node: HTMLElement, visibility_key: string) =>
		Effect.gen(function* () {
			return yield* Effect.acquireRelease(
				Effect.gen(function* () {
					if (item.type !== "user_message" || item.attachments === undefined) return;
					const attachments = item.attachments;
					const supports_intersection_observer = yield* RunBrowserDom(
						() => "IntersectionObserver" in globalThis,
					);
					if (!supports_intersection_observer) {
						const measure_key = `${visibility_key}:measure`;
						const measure = () =>
							image_visibility_runner.ReplaceUnsafe(measure_key, {
								_tag: "Measure",
								attachments,
								node,
								visibility_key,
							});
						yield* RunBrowserDom(() => {
							globalThis.addEventListener("resize", measure);
							globalThis.addEventListener("scroll", measure, true);
						});
						yield* image_visibility_runner.Replace(measure_key, {
							_tag: "Measure",
							attachments,
							node,
							visibility_key,
						});
						return { _tag: "Fallback" as const, attachments, measure, measure_key };
					}
					const observer = yield* RunBrowserDom(() => {
						const observer = new IntersectionObserver(
							(entries) => {
								const visible = entries.some((entry) => entry.isIntersecting);
								image_visibility_runner.ReplaceUnsafe(visibility_key, { _tag: "Visibility", attachments, visible });
							},
							{ rootMargin: "160px 0px" },
						);
						observer.observe(node);
						return observer;
					});
					return { _tag: "Observer" as const, attachments, observer };
				}),
				(resource) =>
					Effect.gen(function* () {
						if (resource === undefined) return;
						if (resource._tag === "Fallback") {
							yield* RunBrowserDom(() => {
								globalThis.removeEventListener("resize", resource.measure);
								globalThis.removeEventListener("scroll", resource.measure, true);
							});
							yield* image_visibility_runner.Release(resource.measure_key);
						} else {
							yield* RunBrowserDom(() => resource.observer.disconnect());
						}
						yield* image_visibility_runner.Replace(visibility_key, {
							_tag: "Visibility",
							attachments: resource.attachments,
							visible: false,
						});
					}),
			);
		});

	const RunImageVisibility = (work: VisibilityWork) =>
		Effect.gen(function* () {
			if (work._tag === "Measure") {
				const { bounds, viewport_height } = yield* RunBrowserDom(() => ({
					bounds: work.node.getBoundingClientRect(),
					viewport_height: globalThis.innerHeight,
				}));
				yield* image_visibility_runner.Replace(work.visibility_key, {
					_tag: "Visibility",
					attachments: work.attachments,
					visible:
						bounds.bottom >= -160 && bounds.top <= viewport_height + 160,
				});
				return;
			}
			if (work._tag === "Observe") {
				yield* ObserveImageVisibility(work.node, work.visibility_key);
				yield* Effect.never;
				return;
			}
			image_group_visible = work.visible;
			if ((work.visible || !image_viewer_open) && onimagevisibilitychange !== undefined) {
				yield* onimagevisibilitychange(work.attachments, work.visible);
			}
		});

	const image_visibility_runner = yield* MakeScopedAttachmentRunner(RunImageVisibility);

	const observe_image_visibility = (node: HTMLElement) => {
		const attachment = image_visibility_runner.Attachment({
			_tag: "Observe",
			node,
			visibility_key: `visibility:${crypto.randomUUID()}`,
		});
		return attachment;
	};

	const CloseImageViewer = () =>
		Effect.gen(function* () {
			if (
				!image_group_visible &&
				item.type === "user_message" &&
				onimagevisibilitychange !== undefined
			) {
				yield* onimagevisibilitychange(item.attachments ?? [], false);
			}
			viewed_image = undefined;
		});
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
							onclick={yield* ViewImage(image)}
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
	<article class="max-w-(--prose-body-width)" aria-label={item.type === "reasoning_summary" ? "Reasoning summary" : "Assistant message"}>
		{#if item.type === "reasoning_summary"}
			<ShimmerText
				class="whitespace-pre-wrap text-base leading-7 text-muted-foreground"
				delay={0}
				duration={2}
			>
				{item.text}
			</ShimmerText>
		{:else}
			<MarkdownContent streaming={item.lifecycle === "streaming"} text={item.text} />
			{#if trailing !== undefined}{@render trailing()}{/if}
		{/if}
	</article>
{/if}

<ImageViewer
	bind:open={image_viewer_open}
	source={viewed_image?.source}
	name={viewed_image?.attachment.name}
	onclose={CloseImageViewer}
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
