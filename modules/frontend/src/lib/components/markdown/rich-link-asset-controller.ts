import { Context, Deferred, Effect, Layer, Ref, Stream } from "effect";

import type { RichLinkAssetMetadata } from "@artisan/protocol";
import { ArtisanClient, type ArtisanClientError } from "@artisan/transport/client";

/** Favicons are deliberately small; retain enough for repeated transcript links, not an image cache. */
export const maximum_retained_rich_link_assets = 32;
export const maximum_retained_rich_link_asset_bytes = 1024 * 1024;
const rich_link_asset_deadline = "2 seconds";

export interface RichLinkAsset {
	readonly bytes: Uint8Array;
	readonly content_type: string;
}

type RichLinkAssetFlight = Deferred.Deferred<RichLinkAsset | undefined, ArtisanClientError>;

interface RichLinkAssetControl {
	readonly entries: ReadonlyMap<string, RichLinkAsset>;
	readonly bytes: number;
	readonly in_flight: ReadonlyMap<string, RichLinkAssetFlight>;
}

type RichLinkAssetClaim =
	| { readonly _tag: "Cached"; readonly asset: RichLinkAsset }
	| { readonly _tag: "Follower"; readonly deferred: RichLinkAssetFlight }
	| { readonly _tag: "Leader"; readonly deferred: RichLinkAssetFlight };

const CopyAsset = (asset: RichLinkAsset): RichLinkAsset => ({
	bytes: asset.bytes.slice(),
	content_type: asset.content_type,
});

const CopyOptionalAsset = (asset: RichLinkAsset | undefined): RichLinkAsset | undefined =>
	asset === undefined ? undefined : CopyAsset(asset);

const AppendAssetChunk = (bytes: Uint8Array, chunk: Uint8Array): Uint8Array => {
	const combined = new Uint8Array(bytes.byteLength + chunk.byteLength);
	combined.set(bytes);
	combined.set(chunk, bytes.byteLength);
	return combined;
};

/**
 * Deduplicates and bounds immutable rich-link asset bytes for the application
 * lifetime. Object URLs are intentionally not retained here: their browser
 * lifetime belongs to the anchor that created them.
 */
export class RichLinkAssetController extends Context.Service<
	RichLinkAssetController,
	{
		readonly Load: (
			favicon: RichLinkAssetMetadata,
		) => Effect.Effect<RichLinkAsset | undefined, ArtisanClientError>;
	}
>()("Artisan/RichLinkAssetController") {}

export const RichLinkAssetControllerLive = Layer.effect(
	RichLinkAssetController,
	Effect.gen(function* () {
		const client = yield* ArtisanClient;
		const controller_scope = yield* Effect.scope;
		const control = yield* Ref.make<RichLinkAssetControl>({
			entries: new Map(),
			bytes: 0,
			in_flight: new Map(),
		});

		const Retain = (asset_id: string, asset: RichLinkAsset) =>
			Ref.update(control, (current) => {
				const entries = new Map(current.entries);
				const replaced = entries.get(asset_id);
				entries.delete(asset_id);
				entries.set(asset_id, asset);
				let bytes =
					current.bytes - (replaced?.bytes.byteLength ?? 0) + asset.bytes.byteLength;
				while (
					entries.size > maximum_retained_rich_link_assets ||
					bytes > maximum_retained_rich_link_asset_bytes
				) {
					const oldest = entries.keys().next().value;
					if (oldest === undefined) break;
					const evicted = entries.get(oldest);
					entries.delete(oldest);
					bytes -= evicted?.bytes.byteLength ?? 0;
				}
				return { ...current, entries, bytes };
			});

		const ReadAsset = (favicon: RichLinkAssetMetadata) =>
			Effect.scoped(
				client
					.OpenAsset(favicon.asset_id)
					.pipe(
						Effect.flatMap((asset) =>
							asset.pipe(Stream.runFold(() => new Uint8Array(), AppendAssetChunk)),
						),
					),
			).pipe(
				Effect.timeoutOption(rich_link_asset_deadline),
				Effect.map((asset) => {
					if (asset._tag === "None") return undefined;
					if (
						asset.value.byteLength !== favicon.bytes ||
						asset.value.byteLength > maximum_retained_rich_link_asset_bytes
					)
						return undefined;
					return { bytes: asset.value, content_type: favicon.content_type };
				}),
			);

		const Complete = (favicon: RichLinkAssetMetadata, deferred: RichLinkAssetFlight) =>
			ReadAsset(favicon).pipe(
				Effect.tap((asset) =>
					asset === undefined ? Effect.void : Retain(favicon.asset_id, asset),
				),
				Effect.exit,
				Effect.flatMap((exit) =>
					Effect.gen(function* () {
						yield* Ref.update(control, (current) => {
							if (current.in_flight.get(favicon.asset_id) !== deferred)
								return current;
							const in_flight = new Map(current.in_flight);
							in_flight.delete(favicon.asset_id);
							return { ...current, in_flight };
						});
						yield* Deferred.done(deferred, exit);
					}),
				),
				Effect.asVoid,
			);

		const Load = (favicon: RichLinkAssetMetadata) =>
			Effect.uninterruptibleMask((restore) =>
				Effect.gen(function* () {
					const candidate = yield* Deferred.make<
						RichLinkAsset | undefined,
						ArtisanClientError
					>();
					const claim = yield* Ref.modify(
						control,
						(current): readonly [RichLinkAssetClaim, RichLinkAssetControl] => {
							const cached = current.entries.get(favicon.asset_id);
							if (cached !== undefined) {
								const entries = new Map(current.entries);
								entries.delete(favicon.asset_id);
								entries.set(favicon.asset_id, cached);
								return [
									{ _tag: "Cached", asset: cached },
									{ ...current, entries },
								];
							}
							const active = current.in_flight.get(favicon.asset_id);
							if (active !== undefined)
								return [{ _tag: "Follower", deferred: active }, current];
							const in_flight = new Map(current.in_flight).set(
								favicon.asset_id,
								candidate,
							);
							return [
								{ _tag: "Leader", deferred: candidate },
								{ ...current, in_flight },
							];
						},
					);
					if (claim._tag === "Cached") return CopyAsset(claim.asset);
					if (claim._tag === "Leader") {
						yield* Effect.forkIn(Complete(favicon, claim.deferred), controller_scope);
					}
					return CopyOptionalAsset(yield* restore(Deferred.await(claim.deferred)));
				}),
			);

		return RichLinkAssetController.of({ Load });
	}),
);
