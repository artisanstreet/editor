import { Context, Data, Effect, Layer, Schema } from "effect";
import * as HttpBody from "effect/unstable/http/HttpBody";
import * as HttpClient from "effect/unstable/http/HttpClient";

import { AdoptForgeEndpoint, ForgeHttpUrl } from "./forge-endpoint";

const PairingHash = Schema.String;

const PairingCode = Schema.String.check(Schema.isMinLength(1), Schema.isMaxLength(256));

const PairingRequest = Schema.Struct({ code: PairingCode });

const PairingResponse = Schema.Struct({ ok: Schema.Boolean });

export class BrowserPairingFailure extends Data.TaggedError("BrowserPairingFailure")<{}> {}

export interface BrowserPairingLocation {
	readonly hash: string;
	readonly pathname: string;
	readonly protocol: string;
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
			/**
			 * The URL is resolved per call: a same-origin page exchanges at its
			 * own `/api/pair`, while the app-scheme renderer targets the adopted
			 * loopback Forge endpoint delivered in the launch fragment.
			 */
			client.post(ForgeHttpUrl("/api/pair"), { body: HttpBody.jsonUnsafe(request) }).pipe(
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
		protocol: browser.location.protocol,
		search: browser.location.search,
	}));
	const ReplaceUrl = (url: string) =>
		Effect.try({
			catch: PairingFailure,
			try: () => browser.history.replaceState(null, "", url),
		});

	return BrowserNavigation.of({ Location, ReplaceUrl });
});

/**
 * Exactly two launch grammars exist: the browser flow's `#pair=<code>` and the
 * installed editor's `#pair=<code>&forge=<endpoint>`. Anything else is treated
 * as absent so an untrusted fragment can never smuggle extra parameters into
 * the exchange.
 */
const pair_hash = /^#pair=([^&=]+)(?:&forge=([^&=]+))?$/;

const PairingFailure = () => new BrowserPairingFailure();

const DecodeFragmentComponent = (encoded: string) =>
	Effect.try({
		catch: PairingFailure,
		try: () => decodeURIComponent(encoded),
	});

const DecodePairingCode = (hash: unknown, page_protocol: string) =>
	Effect.gen(function* () {
		const decoded_hash = yield* Schema.decodeUnknownEffect(PairingHash)(hash).pipe(
			Effect.mapError(PairingFailure),
		);
		const matched = pair_hash.exec(decoded_hash);
		if (matched === null) return undefined;

		const encoded_code = matched[1];
		if (encoded_code === undefined) return undefined;
		if (matched[2] !== undefined) {
			/**
			 * A Forge endpoint travels only in the installed editor's app-scheme
			 * launch. `AdoptForgeEndpoint` refuses HTTP(S) pages, so a crafted
			 * link to a Forge-served page cannot redirect this client — and its
			 * host-scoped session cookie — to a sibling loopback port.
			 */
			const endpoint = yield* DecodeFragmentComponent(matched[2]);
			if (!AdoptForgeEndpoint(endpoint, page_protocol)) return undefined;
		}
		const code = yield* DecodeFragmentComponent(encoded_code);

		return yield* Schema.decodeUnknownEffect(PairingCode)(code).pipe(
			Effect.mapError(PairingFailure),
		);
	});

const DevelopmentPairingCode = Schema.Struct({ code: PairingCode });

/**
 * Development-only self-pairing: the Vite dev server mints a one-time code
 * from the Forge on behalf of the page it served, and the ordinary pairing
 * exchange consumes it — all strictly same-origin. The endpoint exists only
 * inside `vite dev`, so outside development this resolves to `false` without
 * side effects. Callers gate on `import.meta.env.DEV` so production bundles
 * drop the path entirely.
 */
export const AttemptDevelopmentSelfPair: Effect.Effect<boolean> = Effect.tryPromise(async () => {
	const minted = await fetch("/api/dev/pair-code", {
		cache: "no-store",
		method: "POST",
	});
	if (!minted.ok) return false;
	const decoded = Schema.decodeUnknownSync(DevelopmentPairingCode)(await minted.json());
	const paired = await fetch("/api/pair", {
		body: JSON.stringify({ code: decoded.code }),
		headers: { "content-type": "application/json" },
		method: "POST",
	});
	return paired.ok;
}).pipe(Effect.catch(() => Effect.succeed(false)));

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
	const code = yield* DecodePairingCode(location.hash, location.protocol);
	if (code === undefined) return;

	const request = yield* Schema.decodeUnknownEffect(PairingRequest)({ code }).pipe(
		Effect.mapError(PairingFailure),
	);
	const paired = yield* exchange.Pair(request).pipe(Effect.mapError(PairingFailure));
	const decoded_response = yield* Schema.decodeUnknownEffect(PairingResponse)({
		ok: paired,
	}).pipe(Effect.mapError(PairingFailure));
	if (!decoded_response.ok) return yield* Effect.fail(PairingFailure());
	yield* navigation.ReplaceUrl(`${location.pathname}${location.search}`);
});
