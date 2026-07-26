import { Context, Data, Effect, Layer, Schema } from "effect";
import * as HttpBody from "effect/unstable/http/HttpBody";
import * as HttpClient from "effect/unstable/http/HttpClient";

const PairingHash = Schema.String;

const PairingCode = Schema.String.check(Schema.isMinLength(1), Schema.isMaxLength(256));

const PairingRequest = Schema.Struct({ code: PairingCode });

const PairingResponse = Schema.Struct({ ok: Schema.Boolean });

export class BrowserPairingFailure extends Data.TaggedError("BrowserPairingFailure")<{}> {}

export interface BrowserPairingLocation {
	readonly hash: string;
	readonly pathname: string;
	readonly search: string;
}

export class BrowserPairingExchange extends Context.Service<
	BrowserPairingExchange,
	{
		readonly Pair: (
			request: typeof PairingRequest.Type,
		) => Effect.Effect<boolean, BrowserPairingFailure>;
	}
>()("Artisan/BrowserPairingExchange") {}

export const BrowserPairingExchangeLive = Layer.effect(
	BrowserPairingExchange,
	Effect.gen(function* () {
		const client = yield* HttpClient.HttpClient;
		const Pair = (request: typeof PairingRequest.Type) =>
			client.post("/api/pair", { body: HttpBody.jsonUnsafe(request) }).pipe(
				Effect.map((response) => response.status >= 200 && response.status < 300),
				Effect.mapError(PairingFailure),
			);

		return BrowserPairingExchange.of({ Pair });
	}),
);

export class BrowserNavigation extends Context.Service<
	BrowserNavigation,
	{
		readonly Location: Effect.Effect<BrowserPairingLocation>;
		readonly ReplaceUrl: (url: string) => Effect.Effect<void, BrowserPairingFailure>;
	}
>()("Artisan/BrowserNavigation") {}

export const BrowserNavigationLive = Layer.sync(BrowserNavigation, () => {
	const browser = globalThis as unknown as {
		readonly history: { replaceState: (data: null, unused: string, url: string) => void };
		readonly location: BrowserPairingLocation;
	};
	const Location = Effect.sync(() => ({
		hash: browser.location.hash,
		pathname: browser.location.pathname,
		search: browser.location.search,
	}));
	const ReplaceUrl = (url: string) =>
		Effect.try({
			catch: PairingFailure,
			try: () => browser.history.replaceState(null, "", url),
		});

	return BrowserNavigation.of({ Location, ReplaceUrl });
});

const pair_hash = /^#pair=([^&=]+)$/;

const PairingFailure = () => new BrowserPairingFailure();

const DecodePairingCode = (hash: unknown) =>
	Effect.gen(function* () {
		const decoded_hash = yield* Schema.decodeUnknownEffect(PairingHash)(hash).pipe(
			Effect.mapError(PairingFailure),
		);
		const matched = pair_hash.exec(decoded_hash);
		if (matched === null) return undefined;

		const encoded_code = matched[1];
		if (encoded_code === undefined) return undefined;
		const code = yield* Effect.try({
			catch: PairingFailure,
			try: () => decodeURIComponent(encoded_code),
		});

		return yield* Schema.decodeUnknownEffect(PairingCode)(code).pipe(
			Effect.mapError(PairingFailure),
		);
	});

/**
 * Exchanges exactly one URL-fragment pairing capability for a same-origin session
 * before the browser transport is constructed. The capability never escapes this
 * effect and the fragment is removed immediately after a successful exchange.
 */
export const BootstrapBrowserPairing: Effect.Effect<
	void,
	BrowserPairingFailure,
	BrowserNavigation | BrowserPairingExchange
> = Effect.gen(function* () {
	const navigation = yield* BrowserNavigation;
	const exchange = yield* BrowserPairingExchange;
	const location = yield* navigation.Location;
	const code = yield* DecodePairingCode(location.hash);
	if (code === undefined) return;

	const request = yield* Schema.decodeUnknownEffect(PairingRequest)({ code }).pipe(
		Effect.mapError(PairingFailure),
	);
	yield* navigation.ReplaceUrl(`${location.pathname}${location.search}`);
	const paired = yield* exchange.Pair(request).pipe(Effect.mapError(PairingFailure));
	const decoded_response = yield* Schema.decodeUnknownEffect(PairingResponse)({
		ok: paired,
	}).pipe(Effect.mapError(PairingFailure));
	if (!decoded_response.ok) return yield* Effect.fail(PairingFailure());
});
