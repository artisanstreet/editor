<script lang="ts" effect>
	import { ArtisanClient } from "@artisan/transport/client";
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
	let owned_favicon_source: string | undefined;
	let favicon_generation = 0;
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

	const ResolveFaviconAsset = (url: string) =>
		Effect.gen(function* () {
			const resolution = yield* client.ResolveRichLink({ url });
			const favicon = resolution.favicon;
			if (favicon === undefined) return Option.none<FaviconAsset>();

			const asset = yield* client.OpenAsset(favicon.asset_id);
			const bytes = yield* asset.pipe(
				Stream.runFold(() => new Uint8Array(), append_asset_chunk),
			);
			if (bytes.byteLength !== favicon.bytes) return Option.none<FaviconAsset>();

			return Option.some({ bytes, content_type: favicon.content_type });
		});

	const PublishFaviconAsset = (url: string, generation: number, asset: FaviconAsset) =>
		Effect.gen(function* () {
			yield* Effect.uninterruptible(
				Effect.gen(function* () {
					const source = yield* CreateBrowserObjectUrl(asset.bytes, asset.content_type);
					if (generation !== favicon_generation || rich_link_href !== url) {
						yield* ReleaseBrowserObjectUrl(source).pipe(Effect.ignore);
						return;
					}
					owned_favicon_source = source;
					favicon_source = Option.some(source);
				}),
			);
		});

	const RefreshFavicon = (url: string | undefined) =>
		Effect.gen(function* () {
			const generation = (favicon_generation += 1);
			yield* ReleaseOwnedFavicon;
			if (url === undefined) return;

			const resolved = yield* ResolveFaviconAsset(url).pipe(
				Effect.catch(() =>
					Effect.gen(function* () {
						return Option.none<FaviconAsset>();
					}),
				),
			);
			if (Option.isSome(resolved)) yield* PublishFaviconAsset(url, generation, resolved.value);
		});

	/** Link text renders immediately while the component-scoped favicon lookup runs. */
	yield* RefreshFavicon(rich_link_href).pipe(Effect.forkScoped);
</script>

{#if safe_href === undefined}
	{@render children?.()}
{:else}
	<a class="conversation-link" href={safe_href} target="_blank" rel="noopener noreferrer">
		{#if Option.isSome(favicon_source)}
			<img
				class="conversation-link-favicon"
				src={favicon_source.value}
				alt=""
				aria-hidden="true"
				draggable="false"
			/>
		{/if}
		{@render children?.()}
	</a>
{/if}

<style>
	.conversation-link-favicon {
		display: inline-block;
		width: 0.875em;
		height: 0.875em;
		margin-inline-end: 0.28em;
		border-radius: 0.125rem;
		object-fit: contain;
		vertical-align: -0.075em;
	}
</style>
