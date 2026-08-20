import { Cause, Deferred, Effect, Layer, Option, Ref } from "effect";

import {
	RichLinkAssetStore,
	make_in_memory_rich_link_asset_store_layer,
	type RichLinkAssetStoreOptions,
} from "./rich-link-asset-store";
import {
	RichLinkClock,
	RichLinkDnsResolver,
	RichLinkHttpTransport,
	RichLinkMetadata,
	RichLinkMetadataCache,
	RichLinkMetadataError,
	type RichLinkFavicon,
	type RichLinkHttpResponse,
	type RichLinkMetadataDocument,
	type RichLinkMetadataErrorCode,
	type RichLinkMetadataResult,
	type RichLinkResolvedAddress,
} from "./rich-link-metadata";
import { parse_rich_link_html } from "./rich-link-html";
import {
	canonical_hostname,
	ip_address_family,
	is_ip_literal,
	is_localhost_name,
	is_public_address,
} from "./network-policy";
import { NodeRichLinkHttpTransportLive } from "./node-rich-link-transport";
import {
	make_in_memory_rich_link_cache_layer,
	NodeRichLinkDnsResolverLive,
	RichLinkClockLive,
	type InMemoryRichLinkCacheOptions,
} from "./rich-link-infrastructure";

/** Configures external metadata limits and cache policy. */
export interface RichLinkMetadataOptions {
	readonly cache_ttl_ms?: number;
	readonly connect_timeout_ms?: number;
	readonly dns_timeout_ms?: number;
	readonly max_favicon_bytes?: number;
	readonly max_favicon_candidates?: number;
	readonly max_html_bytes?: number;
	readonly max_redirects?: number;
	readonly response_timeout_ms?: number;
}

interface FetchedRichLinkResource {
	readonly content_type: string;
	readonly response: RichLinkHttpResponse;
	readonly url: URL;
}

interface ResolvedFaviconCandidate {
	readonly source: RichLinkFavicon["source"];
	readonly url: URL;
}

type ColdRichLinkMetadataResult = RichLinkMetadataResult & {
	readonly cache: {
		readonly expires_at_ms: number;
		readonly status: "miss";
	};
};

interface ColdResolutionClaim {
	readonly deferred: Deferred.Deferred<ColdRichLinkMetadataResult, RichLinkMetadataError>;
	readonly leader: boolean;
}

type RichLinkFaviconContentType =
	| "image/gif"
	| "image/ico"
	| "image/jpeg"
	| "image/png"
	| "image/vnd.microsoft.icon"
	| "image/webp"
	| "image/x-icon";

const redirect_statuses = new Set([301, 302, 303, 307, 308]);
const html_content_types = new Set(["application/xhtml+xml", "text/html"]);
const favicon_content_types = new Set([
	"image/gif",
	"image/ico",
	"image/jpeg",
	"image/png",
	"image/vnd.microsoft.icon",
	"image/webp",
	"image/x-icon",
]);

const png_signature = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
const gif_87a_signature = new Uint8Array([0x47, 0x49, 0x46, 0x38, 0x37, 0x61]);
const gif_89a_signature = new Uint8Array([0x47, 0x49, 0x46, 0x38, 0x39, 0x61]);
const jpeg_signature = new Uint8Array([0xff, 0xd8, 0xff]);
const ico_signature = new Uint8Array([0x00, 0x00, 0x01, 0x00]);
const riff_signature = new Uint8Array([0x52, 0x49, 0x46, 0x46]);
const webp_signature = new Uint8Array([0x57, 0x45, 0x42, 0x50]);

function metadata_error(url: string, code: RichLinkMetadataErrorCode, cause: unknown) {
	return new RichLinkMetadataError({ cause, code, url });
}

function media_type(headers: Readonly<Record<string, string>>) {
	return headers["content-type"]?.split(";", 1)[0]?.trim().toLowerCase();
}

function starts_with_bytes(body: Uint8Array, signature: Uint8Array, offset = 0) {
	return (
		body.byteLength >= offset + signature.byteLength &&
		signature.every((byte, index) => body[offset + index] === byte)
	);
}

function is_valid_favicon(
	content_type: string,
	body: Uint8Array,
): content_type is RichLinkFaviconContentType {
	switch (content_type) {
		case "image/png":
			return starts_with_bytes(body, png_signature);
		case "image/gif":
			return (
				starts_with_bytes(body, gif_87a_signature) ||
				starts_with_bytes(body, gif_89a_signature)
			);
		case "image/jpeg":
			return starts_with_bytes(body, jpeg_signature);
		case "image/vnd.microsoft.icon":
		case "image/x-icon":
		case "image/ico":
			return starts_with_bytes(body, ico_signature);
		case "image/webp":
			return (
				body.byteLength >= 12 &&
				starts_with_bytes(body, riff_signature) &&
				starts_with_bytes(body, webp_signature, 8)
			);
		default:
			return false;
	}
}

function validate_options(options: Required<RichLinkMetadataOptions>) {
	return (
		Number.isSafeInteger(options.cache_ttl_ms) &&
		options.cache_ttl_ms > 0 &&
		Number.isSafeInteger(options.connect_timeout_ms) &&
		options.connect_timeout_ms > 0 &&
		Number.isSafeInteger(options.dns_timeout_ms) &&
		options.dns_timeout_ms > 0 &&
		Number.isSafeInteger(options.max_favicon_bytes) &&
		options.max_favicon_bytes >= 0 &&
		Number.isSafeInteger(options.max_favicon_candidates) &&
		options.max_favicon_candidates >= 0 &&
		Number.isSafeInteger(options.max_html_bytes) &&
		options.max_html_bytes > 0 &&
		Number.isSafeInteger(options.max_redirects) &&
		options.max_redirects >= 0 &&
		Number.isSafeInteger(options.response_timeout_ms) &&
		options.response_timeout_ms > 0
	);
}

function parse_http_url(input: string, base?: URL) {
	return Effect.gen(function* () {
		const parsed = yield* Effect.try({
			try: () => new URL(input, base),
			catch: (cause) => metadata_error(input, "invalid_url", cause),
		});

		if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
			return yield* Effect.fail(
				metadata_error(parsed.href, "invalid_url", new Error("URL must use HTTP(S)")),
			);
		}

		if (parsed.username || parsed.password) {
			return yield* Effect.fail(
				metadata_error(
					parsed.href,
					"invalid_url",
					new Error("URL credentials are forbidden"),
				),
			);
		}

		const hostname = canonical_hostname(parsed.hostname);

		if (!hostname || is_localhost_name(hostname)) {
			return yield* Effect.fail(
				metadata_error(parsed.href, "blocked_address", new Error("localhost is forbidden")),
			);
		}

		if (is_ip_literal(hostname) && !is_public_address(hostname)) {
			return yield* Effect.fail(
				metadata_error(
					parsed.href,
					"blocked_address",
					new Error("IP address is not globally routable unicast"),
				),
			);
		}

		parsed.hash = "";

		return parsed;
	});
}

function map_dns_error(url: URL, cause: unknown) {
	return metadata_error(url.href, Cause.isTimeoutError(cause) ? "timeout" : "dns", cause);
}

function map_transport_error(url: URL, cause: unknown) {
	if (Cause.isTimeoutError(cause)) {
		return metadata_error(url.href, "timeout", cause);
	}

	if (typeof cause === "object" && cause !== null && "code" in cause) {
		if (cause.code === "connect_timeout" || cause.code === "response_timeout") {
			return metadata_error(url.href, "timeout", cause);
		}

		if (cause.code === "response_size") {
			return metadata_error(url.href, "response_size", cause);
		}
	}

	return metadata_error(url.href, "transport", cause);
}

function response_location(response: RichLinkHttpResponse) {
	return response.headers.location;
}

/** Builds the rich-link resolver from injectable DNS, transport, clock, and cache services. */
export function make_rich_link_metadata_layer(options: RichLinkMetadataOptions = {}) {
	const limits: Required<RichLinkMetadataOptions> = {
		cache_ttl_ms: options.cache_ttl_ms ?? 5 * 60_000,
		connect_timeout_ms: options.connect_timeout_ms ?? 3_000,
		dns_timeout_ms: options.dns_timeout_ms ?? 2_000,
		max_favicon_bytes: options.max_favicon_bytes ?? 128 * 1024,
		max_favicon_candidates: options.max_favicon_candidates ?? 8,
		max_html_bytes: options.max_html_bytes ?? 512 * 1024,
		max_redirects: options.max_redirects ?? 5,
		response_timeout_ms: options.response_timeout_ms ?? 7_000,
	};

	return Layer.effect(
		RichLinkMetadata,
		Effect.gen(function* () {
			if (!validate_options(limits)) {
				return yield* Effect.fail(
					metadata_error("", "configuration", new Error("rich-link limits are invalid")),
				);
			}

			const dns = yield* RichLinkDnsResolver;
			const transport = yield* RichLinkHttpTransport;
			const clock = yield* RichLinkClock;
			const cache = yield* RichLinkMetadataCache;
			const asset_store = yield* RichLinkAssetStore;

			if (limits.max_favicon_bytes > asset_store.limits.max_total_bytes) {
				return yield* Effect.fail(
					metadata_error(
						"",
						"configuration",
						new Error("max_favicon_bytes exceeds the asset-store byte limit"),
					),
				);
			}

			const resolve_address = (url: URL) =>
				Effect.gen(function* () {
					const hostname = canonical_hostname(url.hostname);
					const addresses = is_ip_literal(hostname)
						? ([
								{
									address: hostname,
									family: hostname.includes(":") ? 6 : 4,
								},
							] satisfies ReadonlyArray<RichLinkResolvedAddress>)
						: yield* dns.Resolve(hostname).pipe(
								Effect.timeout(limits.dns_timeout_ms),
								Effect.mapError((cause) => map_dns_error(url, cause)),
							);

					if (addresses.length === 0) {
						return yield* Effect.fail(
							metadata_error(url.href, "dns", new Error("hostname had no addresses")),
						);
					}

					if (
						addresses.some(
							({ address, family }) =>
								!is_public_address(address) ||
								ip_address_family(address) !== family,
						)
					) {
						return yield* Effect.fail(
							metadata_error(
								url.href,
								"blocked_address",
								new Error("DNS returned a non-public address"),
							),
						);
					}

					const address = addresses.at(0);
					if (address === undefined)
						return yield* Effect.fail(
							metadata_error(
								url.href,
								"blocked_address",
								new Error("DNS returned no addresses"),
							),
						);
					return address;
				});

			const request = (url: URL, max_bytes: number, accept: string) =>
				Effect.gen(function* () {
					const pinned_address = yield* resolve_address(url);
					const response = yield* transport
						.Request({
							accept,
							connect_timeout_ms: limits.connect_timeout_ms,
							host_header: url.host,
							max_bytes,
							pinned_address,
							response_timeout_ms: limits.response_timeout_ms,
							tls_server_name: canonical_hostname(url.hostname),
							url: url.href,
						})
						.pipe(
							Effect.timeout(limits.connect_timeout_ms + limits.response_timeout_ms),
							Effect.mapError((cause) => map_transport_error(url, cause)),
						);

					if (response.body.byteLength > max_bytes) {
						return yield* Effect.fail(
							metadata_error(
								url.href,
								"response_size",
								new Error("transport returned an oversized response"),
							),
						);
					}

					return response;
				});

			const fetch_resource = (
				initial_url: URL,
				max_bytes: number,
				accept: string,
				allowed_content_types: ReadonlySet<string>,
			) =>
				Effect.gen(function* () {
					let current = yield* parse_http_url(initial_url.href);
					let redirects = 0;

					while (true) {
						const response = yield* request(current, max_bytes, accept);

						if (redirect_statuses.has(response.status)) {
							const location = response_location(response);

							if (!location) {
								return yield* Effect.fail(
									metadata_error(
										current.href,
										"redirect",
										new Error("redirect response omitted Location"),
									),
								);
							}

							if (redirects >= limits.max_redirects) {
								return yield* Effect.fail(
									metadata_error(
										current.href,
										"redirect",
										new Error("redirect limit exceeded"),
									),
								);
							}

							current = yield* parse_http_url(location, current);
							redirects += 1;

							continue;
						}

						if (response.status < 200 || response.status >= 300) {
							return yield* Effect.fail(
								metadata_error(
									current.href,
									"status",
									new Error(`unexpected HTTP status ${response.status}`),
								),
							);
						}

						const content_type = media_type(response.headers);

						if (!content_type || !allowed_content_types.has(content_type)) {
							return yield* Effect.fail(
								metadata_error(
									current.href,
									"content_type",
									new Error("response content type is not allowed"),
								),
							);
						}

						return {
							content_type,
							response,
							url: current,
						} satisfies FetchedRichLinkResource;
					}
				});

			const resolve_favicon = (candidates: ReadonlyArray<ResolvedFaviconCandidate>) =>
				Effect.gen(function* () {
					for (const candidate of candidates) {
						const fetched = yield* fetch_resource(
							candidate.url,
							limits.max_favicon_bytes,
							"image/*",
							favicon_content_types,
						).pipe(Effect.option);

						if (
							Option.isNone(fetched) ||
							!is_valid_favicon(
								fetched.value.content_type,
								fetched.value.response.body,
							)
						) {
							continue;
						}

						const stored = yield* asset_store
							.Put({
								body: fetched.value.response.body,
								content_type: fetched.value.content_type,
							})
							.pipe(
								Effect.mapError((cause) =>
									metadata_error(fetched.value.url.href, "asset_store", cause),
								),
							);

						return Option.some<RichLinkFavicon>({
							...stored,
							source: candidate.source,
							source_url: fetched.value.url.href,
						});
					}

					return Option.none<RichLinkFavicon>();
				});

			const fetch_document = (requested_url: URL) =>
				Effect.gen(function* () {
					const page = yield* fetch_resource(
						requested_url,
						limits.max_html_bytes,
						"text/html, application/xhtml+xml",
						html_content_types,
					);
					const parsed = yield* Effect.try({
						try: () =>
							parse_rich_link_html(new TextDecoder().decode(page.response.body)),
						catch: (cause) => metadata_error(page.url.href, "parse", cause),
					});
					const base_url = parsed.base_href
						? (URL.parse(parsed.base_href, page.url) ?? page.url)
						: page.url;
					const seen_icons = new Set<string>();
					const icon_candidates: Array<ResolvedFaviconCandidate> = [];

					for (const candidate of parsed.icon_candidates) {
						if (icon_candidates.length >= limits.max_favicon_candidates) {
							break;
						}

						const url = URL.parse(candidate.href, base_url);

						if (!url || seen_icons.has(url.href)) {
							continue;
						}

						seen_icons.add(url.href);
						icon_candidates.push({ source: candidate.source, url });
					}

					const fallback_url = new URL("/favicon.ico", page.url);
					const candidates: Array<ResolvedFaviconCandidate> = [...icon_candidates];

					if (!seen_icons.has(fallback_url.href)) {
						candidates.push({ source: "fallback", url: fallback_url });
					}

					const favicon = yield* resolve_favicon(candidates);
					const fetched_at_ms = yield* clock.Now;
					const site_name = parsed.site_name ?? canonical_hostname(page.url.hostname);
					const page_name = parsed.page_name ?? site_name;

					return {
						favicon,
						fetched_at_ms,
						final_url: page.url.href,
						page_name,
						requested_url: requested_url.href,
						site_name,
						title: Option.fromUndefinedOr(parsed.title),
					} satisfies RichLinkMetadataDocument;
				});

			const service_scope = yield* Effect.scope;
			const cold_resolutions = yield* Ref.make(
				new Map<
					string,
					Deferred.Deferred<ColdRichLinkMetadataResult, RichLinkMetadataError>
				>(),
			);

			const resolve_cold = (cache_key: string) =>
				Effect.uninterruptibleMask((restore) =>
					Effect.gen(function* () {
						const deferred = Deferred.makeUnsafe<
							ColdRichLinkMetadataResult,
							RichLinkMetadataError
						>();
						const resolution = yield* Ref.modify<
							Map<
								string,
								Deferred.Deferred<ColdRichLinkMetadataResult, RichLinkMetadataError>
							>,
							ColdResolutionClaim
						>(cold_resolutions, (current) => {
							const existing = current.get(cache_key);

							if (existing !== undefined) {
								return [{ deferred: existing, leader: false }, current] as const;
							}

							const next = new Map(current);
							next.set(cache_key, deferred);

							return [{ deferred, leader: true }, next] as const;
						});

						if (resolution.leader) {
							const complete = Effect.gen(function* () {
								const result = yield* Effect.exit(
									Effect.gen(function* () {
										const document = yield* fetch_document(new URL(cache_key));
										const expires_at_ms =
											document.fetched_at_ms + limits.cache_ttl_ms;

										yield* cache.Put(cache_key, { document, expires_at_ms });

										return {
											...document,
											cache: { expires_at_ms, status: "miss" as const },
										};
									}),
								);

								yield* Ref.update(cold_resolutions, (current) => {
									if (current.get(cache_key) !== resolution.deferred) {
										return current;
									}

									const next = new Map(current);
									next.delete(cache_key);

									return next;
								});
								return yield* Deferred.done(resolution.deferred, result);
							});

							yield* Effect.forkIn(complete, service_scope, {
								uninterruptible: false,
							});
						}

						return yield* restore(Deferred.await(resolution.deferred));
					}),
				);

			const resolve = (input: string) =>
				Effect.gen(function* () {
					const requested_url = yield* parse_http_url(input);
					const cache_key = requested_url.href;
					const now = yield* clock.Now;
					const cached = yield* cache.Get(cache_key);

					if (Option.isSome(cached) && cached.value.expires_at_ms > now) {
						const cached_favicon = cached.value.document.favicon;
						const asset_available = Option.isNone(cached_favicon)
							? true
							: Option.isSome(yield* asset_store.Get(cached_favicon.value.asset_id));

						if (asset_available) {
							return {
								...cached.value.document,
								cache: {
									expires_at_ms: cached.value.expires_at_ms,
									status: "hit" as const,
								},
							};
						}
					}

					/**
					 * The layer-owned Deferred shares only active misses. Its scoped worker
					 * remains alive when an individual route stops waiting, while every DNS
					 * and transport operation retains its existing deadline.
					 */
					return yield* resolve_cold(cache_key);
				});

			return { Resolve: resolve };
		}),
	);
}

/** Configures production rich-link metadata, cache, and retained-asset limits. */
export interface NodeRichLinkMetadataOptions extends RichLinkMetadataOptions {
	readonly asset_store?: RichLinkAssetStoreOptions;
	readonly metadata_cache?: InMemoryRichLinkCacheOptions;
}

/** Provides production metadata resolution and the retained-asset read seam. */
export function make_node_rich_link_metadata_layer(options: NodeRichLinkMetadataOptions = {}) {
	const asset_store = make_in_memory_rich_link_asset_store_layer(options.asset_store);
	const infrastructure = Layer.mergeAll(
		NodeRichLinkDnsResolverLive,
		NodeRichLinkHttpTransportLive,
		RichLinkClockLive,
		make_in_memory_rich_link_cache_layer(options.metadata_cache),
	);
	const metadata = make_rich_link_metadata_layer(options).pipe(Layer.provide(infrastructure));

	return metadata.pipe(Layer.provideMerge(asset_store));
}
