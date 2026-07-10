import { lookup } from "node:dns/promises";
import { isIP } from "node:net";

import { Effect, Layer, Option, Ref } from "effect";

import {
	RichLinkClock,
	RichLinkDnsError,
	RichLinkDnsResolver,
	RichLinkMetadataCache,
	RichLinkMetadataError,
	type RichLinkCacheEntry,
	type RichLinkResolvedAddress,
} from "./rich-link-metadata";

/** Configures the bounded process-local metadata cache. */
export interface InMemoryRichLinkCacheOptions {
	readonly max_entries?: number;
}

/** Provides wall-clock epoch milliseconds for production metadata caching. */
export const RichLinkClockLive = Layer.succeed(RichLinkClock, {
	Now: Effect.sync(() => Date.now()),
});

/** Builds an isolated in-memory rich-link metadata cache. */
export function make_in_memory_rich_link_cache_layer(options: InMemoryRichLinkCacheOptions = {}) {
	const max_entries = options.max_entries ?? 512;

	return Layer.effect(
		RichLinkMetadataCache,
		Effect.gen(function* () {
			if (!Number.isSafeInteger(max_entries) || max_entries <= 0) {
				return yield* Effect.fail(
					new RichLinkMetadataError({
						cause: new Error(
							"metadata cache max_entries must be a positive safe integer",
						),
						code: "configuration",
						url: "",
					}),
				);
			}

			const entries = yield* Ref.make(new Map<string, RichLinkCacheEntry>());

			return {
				Get: (key) =>
					Ref.get(entries).pipe(
						Effect.map((current) => Option.fromUndefinedOr(current.get(key))),
					),
				Put: (key, entry) =>
					Ref.update(entries, (current) => {
						const next = new Map(current);

						next.delete(key);

						if (next.size >= max_entries) {
							const oldest_key = next.keys().next().value;

							if (oldest_key !== undefined) {
								next.delete(oldest_key);
							}
						}

						next.set(key, entry);

						return next;
					}),
			};
		}),
	);
}

/** Provides an empty process-local cache for production composition. */
export const RichLinkMetadataCacheLive = make_in_memory_rich_link_cache_layer();

/** Resolves all addresses once so the caller can validate and pin one result. */
export const NodeRichLinkDnsResolverLive = Layer.succeed(RichLinkDnsResolver, {
	Resolve: (hostname) => {
		const family = isIP(hostname);

		if (family === 4 || family === 6) {
			return Effect.succeed([
				{ address: hostname, family },
			] satisfies ReadonlyArray<RichLinkResolvedAddress>);
		}

		return Effect.tryPromise({
			try: () => lookup(hostname, { all: true, verbatim: true }),
			catch: (cause) => new RichLinkDnsError({ cause, hostname }),
		}).pipe(
			Effect.map((addresses) =>
				addresses.map(({ address, family }) => ({
					address,
					family: family === 6 ? 6 : 4,
				})),
			),
		);
	},
});
