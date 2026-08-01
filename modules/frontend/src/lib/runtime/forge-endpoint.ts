import { Context, Data, Effect, Layer } from "effect";

/** Browser-storage failures stay typed at the renderer capability boundary. */
export class ForgeEndpointStorageError extends Data.TaggedError("ForgeEndpointStorageError")<{
	readonly cause: unknown;
}> {}

const storage_key = "artisan.forge-endpoint";

export class ForgeEndpointStore extends Context.Service<
	ForgeEndpointStore,
	{
		readonly Read: Effect.Effect<string | undefined, ForgeEndpointStorageError>;
		readonly Write: (endpoint: string) => Effect.Effect<void, ForgeEndpointStorageError>;
	}
>()("Artisan/ForgeEndpointStore") {}

/**
 * Browser session storage is an I/O capability. Privacy modes may deny it, in
 * which case endpoint adoption remains memoryless rather than throwing across
 * component work.
 */
export const ForgeEndpointStoreLive = Layer.effect(
	ForgeEndpointStore,
	Effect.gen(function* () {
		const Read = Effect.gen(function* () {
			return yield* Effect.try({
				try: () => globalThis.sessionStorage?.getItem(storage_key) ?? undefined,
				catch: (cause) => new ForgeEndpointStorageError({ cause }),
			});
		});
		const Write = (endpoint: string) =>
			Effect.gen(function* () {
				yield* Effect.try({
					try: () => globalThis.sessionStorage?.setItem(storage_key, endpoint),
					catch: (cause) => new ForgeEndpointStorageError({ cause }),
				});
			});

		return ForgeEndpointStore.of({ Read, Write });
	}),
);

const endpoint_bearing_page = (page_protocol: string) =>
	page_protocol !== "http:" && page_protocol !== "https:";

/** Accepts only an uncredentialed loopback HTTP origin with an explicit port. */
export const DecodeLoopbackForgeEndpoint = (candidate: unknown): string | undefined => {
	if (typeof candidate !== "string" || candidate.length === 0 || candidate.length > 256) {
		return undefined;
	}
	try {
		const url = new URL(candidate);
		const loopback =
			url.hostname === "127.0.0.1" || url.hostname === "[::1]" || url.hostname === "::1";
		if (
			url.protocol !== "http:" ||
			!loopback ||
			url.username !== "" ||
			url.password !== "" ||
			url.port === ""
		) {
			return undefined;
		}
		return url.origin;
	} catch {
		return undefined;
	}
};

/** Adopts a validated loopback endpoint for this browser session. */
export const AdoptForgeEndpoint = (candidate: string, page_protocol: string) =>
	Effect.gen(function* () {
		if (!endpoint_bearing_page(page_protocol)) return false;
		const endpoint = DecodeLoopbackForgeEndpoint(candidate);
		if (endpoint === undefined) return false;
		const store = yield* ForgeEndpointStore;
		yield* store
			.Write(endpoint)
			.pipe(Effect.catchTag("ForgeEndpointStorageError", () => Effect.gen(function* () {})));
		return true;
	});

/** The adopted loopback Forge origin, or undefined on ordinary same-origin pages. */
export const ResolveForgeEndpoint = Effect.gen(function* () {
	const store = yield* ForgeEndpointStore;
	const stored = yield* store.Read.pipe(
		Effect.catchTag("ForgeEndpointStorageError", () =>
			Effect.gen(function* () {
				return undefined;
			}),
		),
	);
	return stored === undefined ? undefined : DecodeLoopbackForgeEndpoint(stored);
});

/** Builds a Forge HTTP URL: absolute against the adopted endpoint, else same-origin relative. */
export const ForgeHttpUrl = (path: string) =>
	Effect.gen(function* () {
		const endpoint = yield* ResolveForgeEndpoint;
		return endpoint === undefined ? path : `${endpoint}${path}`;
	});
