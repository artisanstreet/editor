import { createHash } from "node:crypto";

import { Context, Data, Effect, Layer, Option, Ref } from "effect";

/** Identifies a bounded rich-link asset-store failure. */
export type RichLinkAssetStoreErrorCode = "asset_too_large" | "configuration";

/** Reports why a rich-link asset could not be retained. */
export class RichLinkAssetStoreError extends Data.TaggedError("RichLinkAssetStoreError")<{
	readonly asset_id: string;
	readonly cause: unknown;
	readonly code: RichLinkAssetStoreErrorCode;
}> {}

/** Defines the retained entry and byte ceilings for a rich-link asset store. */
export interface RichLinkAssetStoreLimits {
	readonly max_entries: number;
	readonly max_total_bytes: number;
}

/** Configures an isolated bounded in-memory rich-link asset store. */
export interface RichLinkAssetStoreOptions {
	readonly max_entries?: number;
	readonly max_total_bytes?: number;
}

/** Supplies verified bytes and media type for content-addressed storage. */
export interface RichLinkAssetStoreInput {
	readonly body: Uint8Array;
	readonly content_type: string;
}

/** Describes one retained asset without exposing its binary body. */
export interface RichLinkAssetMetadata {
	readonly asset_id: string;
	readonly bytes: number;
	readonly content_type: string;
}

/** Contains one retained asset returned through the future binary read seam. */
export interface RichLinkAsset extends RichLinkAssetMetadata {
	readonly body: Uint8Array;
}

/** Stores and reads bounded content-addressed rich-link assets. */
export class RichLinkAssetStore extends Context.Service<
	RichLinkAssetStore,
	{
		readonly Get: (asset_id: string) => Effect.Effect<Option.Option<RichLinkAsset>>;
		readonly limits: RichLinkAssetStoreLimits;
		readonly Put: (
			input: RichLinkAssetStoreInput,
		) => Effect.Effect<RichLinkAssetMetadata, RichLinkAssetStoreError>;
	}
>()("Artisan/RichLinkAssetStore") {}

interface RichLinkAssetStoreState {
	readonly entries: ReadonlyMap<string, RichLinkAsset>;
	readonly retained_bytes: number;
}

function asset_store_error(code: RichLinkAssetStoreErrorCode, cause: unknown, asset_id = "") {
	return new RichLinkAssetStoreError({ asset_id, cause, code });
}

function validate_limits(limits: RichLinkAssetStoreLimits) {
	return (
		Number.isSafeInteger(limits.max_entries) &&
		limits.max_entries > 0 &&
		Number.isSafeInteger(limits.max_total_bytes) &&
		limits.max_total_bytes > 0
	);
}

function asset_metadata(asset: RichLinkAsset): RichLinkAssetMetadata {
	return {
		asset_id: asset.asset_id,
		bytes: asset.bytes,
		content_type: asset.content_type,
	};
}

/** Builds an isolated LRU asset store bounded by retained entries and bytes. */
export function make_in_memory_rich_link_asset_store_layer(
	options: RichLinkAssetStoreOptions = {},
) {
	const limits: RichLinkAssetStoreLimits = {
		max_entries: options.max_entries ?? 256,
		max_total_bytes: options.max_total_bytes ?? 16 * 1024 * 1024,
	};

	return Layer.effect(
		RichLinkAssetStore,
		Effect.gen(function* () {
			if (!validate_limits(limits)) {
				return yield* Effect.fail(
					asset_store_error(
						"configuration",
						new Error("asset-store limits must be positive safe integers"),
					),
				);
			}

			const state = yield* Ref.make<RichLinkAssetStoreState>({
				entries: new Map(),
				retained_bytes: 0,
			});

			const get = (asset_id: string) =>
				Ref.modify(state, (current) => {
					const asset = current.entries.get(asset_id);

					if (!asset) {
						return [Option.none<RichLinkAsset>(), current] as const;
					}

					const entries = new Map(current.entries);

					entries.delete(asset_id);
					entries.set(asset_id, asset);

					return [
						Option.some({ ...asset, body: asset.body.slice() }),
						{ ...current, entries },
					] as const;
				});

			const put = (input: RichLinkAssetStoreInput) =>
				Effect.gen(function* () {
					if (input.body.byteLength > limits.max_total_bytes) {
						return yield* Effect.fail(
							asset_store_error(
								"asset_too_large",
								new Error("asset exceeds the store byte limit"),
							),
						);
					}

					const body = input.body.slice();
					const asset_id = createHash("sha256").update(body).digest("hex");
					const stored = yield* Ref.modify(state, (current) => {
						const existing = current.entries.get(asset_id);
						const entries = new Map(current.entries);
						let retained_bytes = current.retained_bytes;

						if (existing) {
							entries.delete(asset_id);
							retained_bytes -= existing.bytes;
						}

						while (
							entries.size >= limits.max_entries ||
							retained_bytes + body.byteLength > limits.max_total_bytes
						) {
							const oldest_asset_id = entries.keys().next().value;

							if (oldest_asset_id === undefined) {
								break;
							}

							const evicted = entries.get(oldest_asset_id);

							entries.delete(oldest_asset_id);
							retained_bytes -= evicted?.bytes ?? 0;
						}

						const asset: RichLinkAsset = {
							asset_id,
							body,
							bytes: body.byteLength,
							content_type: input.content_type,
						};

						entries.set(asset_id, asset);

						return [
							asset_metadata(asset),
							{
								entries,
								retained_bytes: retained_bytes + asset.bytes,
							},
						] as const;
					});

					return stored;
				});

			return { Get: get, limits, Put: put };
		}),
	);
}

/** Provides a bounded process-local rich-link asset store. */
export const RichLinkAssetStoreLive = make_in_memory_rich_link_asset_store_layer();
