<script lang="ts" effect>
	import type { RichLinkFavicon } from "@artisan/protocol";
	import { ArtisanClient } from "@artisan/transport/client";
	import World from "@tabler/icons-svelte/icons/world";
	import { Effect, Option, Stream } from "effect";
	import type { Snippet } from "svelte";
	import {
		CreateBrowserObjectUrl,
		ReleaseBrowserObjectUrl,
	} from "$lib/browser/object-url";
	import { rich_link_metadata_url } from "./link-url";

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

	const append_asset_chunk = (bytes: Uint8Array, chunk: Uint8Array): Uint8Array => {
		const combined = new Uint8Array(bytes.byteLength + chunk.byteLength);
		combined.set(bytes);
		combined.set(chunk, bytes.byteLength);
		return combined;
	};

	const client = yield* ArtisanClient;
	let favicon_source = $state(Option.none<string>());
	let resolved_title = $state(Option.none<string>());
	let show_web_fallback = $state(false);
	let owned_favicon_source: string | undefined;
	let rich_link_generation = 0;
	type FaviconAsset = {
		readonly bytes: Uint8Array;
		readonly content_type: string;
	};

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

	const ResolveFaviconAsset = (favicon: RichLinkFavicon) =>
		Effect.gen(function* () {
			const asset = yield* client.OpenAsset(favicon.asset_id);
			const bytes = yield* asset.pipe(
				Stream.runFold(() => new Uint8Array(), append_asset_chunk),
			);
			if (bytes.byteLength !== favicon.bytes) return Option.none<FaviconAsset>();

			return Option.some({ bytes, content_type: favicon.content_type });
		});
	const ResolveFaviconAssetForPresentation = (favicon: RichLinkFavicon) =>
		Effect.gen(function* () {
			const result = yield* ResolveFaviconAsset(favicon).pipe(
				Effect.timeoutOption("2 seconds"),
			);
			return Option.flatten(result);
		});

	const PublishFaviconAsset = (url: string, generation: number, asset: FaviconAsset) =>
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

			const resolution = yield* client.ResolveRichLink({ url }).pipe(Effect.option);
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
			if (Option.isNone(asset)) {
				show_web_fallback = true;
				return;
			}

			yield* PublishFaviconAsset(url, generation, asset.value);
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
	.conversation-link-favicon {
		display: inline-block;
		width: 0.875em;
		height: 0.875em;
		max-width: none;
		margin-block: 0;
		margin-inline: 0;
		border-radius: 0.125rem;
		object-fit: contain;
		vertical-align: -0.075em;
	}

	.conversation-link-web-fallback {
		display: inline-flex;
		width: 0.875em;
		height: 0.875em;
		margin-block: 0;
		margin-inline: 0;
		align-items: center;
		justify-content: center;
		vertical-align: -0.075em;
	}
</style>
