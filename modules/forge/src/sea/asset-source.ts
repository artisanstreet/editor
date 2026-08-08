import * as sea from "node:sea";

import { Context, Data, Effect, Layer } from "effect";

export class SeaAssetUnavailable extends Data.TaggedError("SeaAssetUnavailable")<{
	readonly asset_id: string;
	readonly cause: unknown;
}> {}

export interface SeaAssetSourceShape {
	readonly Read: (asset_id: string) => Effect.Effect<Uint8Array, SeaAssetUnavailable>;
}

/** The narrow boundary around Node's SEA-only asset API, replaceable in tests. */
export class SeaAssetSource extends Context.Service<SeaAssetSource, SeaAssetSourceShape>()(
	"Artisan/SeaAssetSource",
) {}

export const NodeSeaAssetSourceLive = Layer.succeed(
	SeaAssetSource,
	SeaAssetSource.of({
		Read: (asset_id) =>
			Effect.try({
				catch: (cause) => new SeaAssetUnavailable({ asset_id, cause }),
				try: () => new Uint8Array(sea.getRawAsset(asset_id)),
			}),
	}),
);
