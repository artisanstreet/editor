<script lang="ts" effect>
	import type { RichLinkFavicon } from "@artisan/protocol";
	import World from "@tabler/icons-svelte/icons/world";
	import { Effect, Option } from "effect";
	import type { Snippet } from "svelte";
	import {
		CreateBrowserObjectUrl,
		ReleaseBrowserObjectUrl,
	} from "$lib/browser/object-url";
	import { rich_link_metadata_url } from "./link-url";
	import {
	type RichLinkAsset,
	RichLinkAssetController,
	} from "./rich-link-asset-controller";
	import { RichLinkMetadataController } from "./rich-link-metadata-controller";

	let {
		children,
		href,
	}: {
		children?: Snippet;
		href?: string;
	} = $props();

	/**
	 * Assistant links are untrusted, so only well-known protocols become live
	 * anchors; anything else renders as plain text. markdown-it already vets
	 * link protocols during parsing, so this guard is defense in depth.
	 */
	const safe_href = $derived.by(() => {
		if (href === undefined) return undefined;
		try {
			const protocol = new URL(href, "https://conversation.invalid").protocol;
			return protocol === "https:" || protocol === "http:" || protocol === "mailto:"
				? href
				: undefined;
		} catch {
			return undefined;
		}
	});

	const rich_link_href = $derived(Option.getOrUndefined(rich_link_metadata_url(safe_href)));

	const rich_link_assets = yield* RichLinkAssetController;
	const rich_link_metadata = yield* RichLinkMetadataController;
	let favicon_source = $state(Option.none<string>());
	let resolved_title = $state(Option.none<string>());
	let show_web_fallback = $state(false);
	let owned_favicon_source: string | undefined;
	let rich_link_generation = 0;

	const ReleaseOwnedFavicon = Effect.gen(function* () {
		const source = owned_favicon_source;
		owned_favicon_source = undefined;
		favicon_source = Option.none();
		if (source !== undefined) {
			yield* ReleaseBrowserObjectUrl(source).pipe(Effect.ignore);
		}
	});

	yield* Effect.addFinalizer(() =>
		Effect.gen(function* () {
			yield* ReleaseOwnedFavicon;
		}),
	);

	const ResolveFaviconAssetForPresentation = (favicon: RichLinkFavicon) =>
		rich_link_assets.Load(favicon).pipe(
			Effect.timeoutOption("2 seconds"),
			Effect.map(Option.getOrUndefined),
		);

	const PublishFaviconAsset = (url: string, generation: number, asset: RichLinkAsset) =>
		Effect.gen(function* () {
			yield* Effect.uninterruptible(
				Effect.gen(function* () {
					const source = yield* CreateBrowserObjectUrl(asset.bytes, asset.content_type);
					if (generation !== rich_link_generation || rich_link_href !== url) {
						yield* ReleaseBrowserObjectUrl(source).pipe(Effect.ignore);
						return;
					}
					owned_favicon_source = source;
					favicon_source = Option.some(source);
					show_web_fallback = false;
				}),
			);
		});

	const RefreshRichLink = (url: string | undefined) =>
		Effect.gen(function* () {
			const generation = (rich_link_generation += 1);
			yield* ReleaseOwnedFavicon;
			resolved_title = Option.none();
			show_web_fallback = url !== undefined;
			if (url === undefined) return;

			const resolution = yield* rich_link_metadata.Load(url).pipe(Effect.option);
			if (generation !== rich_link_generation || rich_link_href !== url) return;
			if (Option.isNone(resolution)) {
				show_web_fallback = true;
				return;
			}

			resolved_title = Option.some(resolution.value.page_name);
			const favicon = resolution.value.favicon;
			if (favicon === undefined) {
				show_web_fallback = true;
				return;
			}

			const asset = yield* ResolveFaviconAssetForPresentation(favicon);
			if (generation !== rich_link_generation || rich_link_href !== url) return;
			if (asset === undefined) {
				show_web_fallback = true;
				return;
			}

			yield* PublishFaviconAsset(url, generation, asset);
		});

	const HideFailedFavicon = (source: string) =>
		Effect.gen(function* () {
			if (owned_favicon_source !== source) return;
			yield* ReleaseOwnedFavicon;
			show_web_fallback = true;
		});

	/** Authored link text renders immediately while the resolved presentation loads. */
	yield* RefreshRichLink(rich_link_href).pipe(Effect.forkScoped);
</script>

{#if safe_href === undefined}
	{@render children?.()}
{:else}
	<a
		class="conversation-link"
		href={safe_href}
		title={safe_href}
		target="_blank"
		rel="noopener noreferrer"
	>
		{#if Option.isSome(favicon_source)}
			<img
				class="conversation-link-favicon"
				src={favicon_source.value}
				alt=""
				aria-hidden="true"
				draggable="false"
				onerror={yield* HideFailedFavicon(favicon_source.value)}
			/>
		{:else if show_web_fallback}
			<span class="conversation-link-web-fallback" aria-hidden="true">
				<World size="1em" stroke={1.8} />
			</span>
		{/if}
		{#if Option.isSome(resolved_title)}
			{resolved_title.value}
		{:else}
			{@render children?.()}
		{/if}
	</a>
{/if}

<style>
	/**
	 * Conversation prose gives ordinary images block layout and vertical
	 * margins. Keep link metadata in the surrounding inline formatting context
	 * with component-scoped specificity so its icon cannot split from its text.
	 *
	 * The anchor aligns its items on the link text's baseline so the link sits
	 * in the surrounding line, which would otherwise hang both icons off that
	 * baseline. Centring them against the line box instead lands them on the
	 * text's optical centre.
	 */
	.conversation-link-favicon {
		display: block;
		flex: none;
		align-self: center;
		width: 0.875em;
		height: 0.875em;
		max-width: none;
		margin-block: 0;
		margin-inline: 0;
		border-radius: 0.125rem;
		object-fit: contain;
	}

	.conversation-link-web-fallback {
		display: inline-flex;
		width: 0.875em;
		height: 0.875em;
		margin-block: 0;
		margin-inline: 0;
		align-items: center;
		align-self: center;
		justify-content: center;
		flex: none;
	}
</style>
