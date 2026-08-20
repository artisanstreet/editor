import { Clock, Context, Deferred, Effect, Layer, Ref } from "effect";

import type { RichLinkResolution } from "@artisan/protocol";
import { ArtisanClient, type ArtisanClientError } from "@artisan/transport/client";

export const maximum_retained_rich_link_metadata = 64;

type RichLinkMetadataEntry = {
	readonly expires_at_ms: number;
	readonly fetched_at_ms: number;
	readonly resolution: RichLinkResolution;
};

type RichLinkMetadataFlight = Deferred.Deferred<RichLinkResolution, ArtisanClientError>;

type RichLinkMetadataControl = {
	readonly entries: ReadonlyMap<string, RichLinkMetadataEntry>;
	readonly in_flight: ReadonlyMap<string, RichLinkMetadataFlight>;
};

type RichLinkMetadataClaim =
	| { readonly _tag: "Cached"; readonly resolution: RichLinkResolution }
	| { readonly _tag: "Follower"; readonly deferred: RichLinkMetadataFlight }
	| { readonly _tag: "Leader"; readonly deferred: RichLinkMetadataFlight };

const CanonicalUrl = (url: string) => {
	const canonical = new URL(url);
	canonical.hash = "";
	return canonical.href;
};

/** Retains backend-authoritative rich-link metadata by canonical requested URL. */
export class RichLinkMetadataController extends Context.Service<
	RichLinkMetadataController,
	{
		readonly Load: (url: string) => Effect.Effect<RichLinkResolution, ArtisanClientError>;
	}
>()("Artisan/RichLinkMetadataController") {}

export const RichLinkMetadataControllerLive = Layer.effect(
	RichLinkMetadataController,
	Effect.gen(function* () {
		const client = yield* ArtisanClient;
		const controller_scope = yield* Effect.scope;
		const control = yield* Ref.make<RichLinkMetadataControl>({
			entries: new Map(),
			in_flight: new Map(),
		});

		const Retain = (url: string, resolution: RichLinkResolution) =>
			Ref.modify(control, (current) => {
				const expires_at_ms = Date.parse(resolution.cache.expires_at);
				const fetched_at_ms = Date.parse(resolution.fetched_at);
				if (!Number.isFinite(expires_at_ms) || !Number.isFinite(fetched_at_ms)) {
					return [resolution, current] as const;
				}
				const previous = current.entries.get(url);
				if (previous !== undefined && previous.fetched_at_ms > fetched_at_ms) {
					return [previous.resolution, current] as const;
				}
				const entries = new Map(current.entries);
				entries.delete(url);
				entries.set(url, { expires_at_ms, fetched_at_ms, resolution });
				while (entries.size > maximum_retained_rich_link_metadata) {
					const oldest = entries.keys().next().value;
					if (oldest === undefined) break;
					entries.delete(oldest);
				}
				return [resolution, { ...current, entries }] as const;
			});

		const Complete = (url: string, deferred: RichLinkMetadataFlight) =>
			client.ResolveRichLink({ url }).pipe(
				Effect.flatMap((resolution) => Retain(url, resolution)),
				Effect.exit,
				Effect.flatMap((exit) =>
					Effect.gen(function* () {
						yield* Ref.update(control, (current) => {
							if (current.in_flight.get(url) !== deferred) return current;
							const in_flight = new Map(current.in_flight);
							in_flight.delete(url);
							return { ...current, in_flight };
						});
						yield* Deferred.done(deferred, exit);
					}),
				),
				Effect.asVoid,
			);

		const Load = (input: string) =>
			Effect.uninterruptibleMask((restore) =>
				Effect.gen(function* () {
					const url = CanonicalUrl(input);
					const now_ms = yield* Clock.currentTimeMillis;
					const candidate = yield* Deferred.make<
						RichLinkResolution,
						ArtisanClientError
					>();
					const claim = yield* Ref.modify(
						control,
						(current): readonly [RichLinkMetadataClaim, RichLinkMetadataControl] => {
							const cached = current.entries.get(url);
							if (cached !== undefined && cached.expires_at_ms > now_ms) {
								const entries = new Map(current.entries);
								entries.delete(url);
								entries.set(url, cached);
								return [
									{ _tag: "Cached", resolution: cached.resolution },
									{ ...current, entries },
								];
							}
							const active = current.in_flight.get(url);
							if (active !== undefined)
								return [{ _tag: "Follower", deferred: active }, current];
							const in_flight = new Map(current.in_flight).set(url, candidate);
							return [
								{ _tag: "Leader", deferred: candidate },
								{ ...current, in_flight },
							];
						},
					);
					if (claim._tag === "Cached") return claim.resolution;
					if (claim._tag === "Leader") {
						yield* Effect.forkIn(Complete(url, claim.deferred), controller_scope);
					}
					return yield* restore(Deferred.await(claim.deferred));
				}),
			);

		return RichLinkMetadataController.of({ Load });
	}),
);
