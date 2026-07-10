import { Context, Data, Effect, Option } from "effect";

/** Identifies the boundary that rejected or failed a rich-link request. */
export type RichLinkMetadataErrorCode =
	| "asset_store"
	| "blocked_address"
	| "cache"
	| "configuration"
	| "content_type"
	| "dns"
	| "invalid_url"
	| "parse"
	| "redirect"
	| "response_size"
	| "status"
	| "timeout"
	| "transport";

/** Reports a safe, provider-neutral rich-link metadata failure. */
export class RichLinkMetadataError extends Data.TaggedError("RichLinkMetadataError")<{
	readonly cause: unknown;
	readonly code: RichLinkMetadataErrorCode;
	readonly url: string;
}> {}

/** Reports a DNS adapter failure before any socket is opened. */
export class RichLinkDnsError extends Data.TaggedError("RichLinkDnsError")<{
	readonly cause: unknown;
	readonly hostname: string;
}> {}

/** Identifies a bounded HTTP transport failure. */
export type RichLinkTransportErrorCode =
	| "connect_timeout"
	| "request"
	| "response_size"
	| "response_timeout"
	| "unsupported_encoding";

/** Reports a failure from the pinned-address HTTP transport. */
export class RichLinkTransportError extends Data.TaggedError("RichLinkTransportError")<{
	readonly cause: unknown;
	readonly code: RichLinkTransportErrorCode;
	readonly url: string;
}> {}

/** Describes one DNS result that can be pinned by the HTTP transport. */
export interface RichLinkResolvedAddress {
	readonly address: string;
	readonly family: 4 | 6;
}

/** Defines one bounded request to an already validated and pinned address. */
export interface RichLinkHttpRequest {
	readonly accept: string;
	readonly connect_timeout_ms: number;
	readonly host_header: string;
	readonly max_bytes: number;
	readonly pinned_address: RichLinkResolvedAddress;
	readonly response_timeout_ms: number;
	readonly tls_server_name: string;
	readonly url: string;
}

/** Contains a bounded response body and normalized response headers. */
export interface RichLinkHttpResponse {
	readonly body: Uint8Array;
	readonly headers: Readonly<Record<string, string>>;
	readonly status: number;
}

/** Resolves hostnames without making policy decisions about returned addresses. */
export class RichLinkDnsResolver extends Context.Service<
	RichLinkDnsResolver,
	{
		readonly Resolve: (
			hostname: string,
		) => Effect.Effect<ReadonlyArray<RichLinkResolvedAddress>, RichLinkDnsError>;
	}
>()("Artisan/RichLinkDnsResolver") {}

/**
 * Executes HTTP requests against the supplied pinned address while preserving
 * the supplied Host header and TLS server name. Implementations must not resolve
 * the URL hostname independently.
 */
export class RichLinkHttpTransport extends Context.Service<
	RichLinkHttpTransport,
	{
		readonly Request: (
			input: RichLinkHttpRequest,
		) => Effect.Effect<RichLinkHttpResponse, RichLinkTransportError>;
	}
>()("Artisan/RichLinkHttpTransport") {}

/** Supplies epoch milliseconds for cache and result timestamps. */
export class RichLinkClock extends Context.Service<
	RichLinkClock,
	{
		readonly Now: Effect.Effect<number>;
	}
>()("Artisan/RichLinkClock") {}

/** Describes a fetched favicon without exposing its untrusted bytes. */
export interface RichLinkFavicon {
	readonly asset_id: string;
	readonly bytes: number;
	readonly content_type: string;
	readonly source: "apple_touch" | "document_icon" | "fallback";
	readonly source_url: string;
}

/** Contains normalized metadata extracted from one external document. */
export interface RichLinkMetadataDocument {
	readonly favicon: Option.Option<RichLinkFavicon>;
	readonly fetched_at_ms: number;
	readonly final_url: string;
	readonly page_name: string;
	readonly requested_url: string;
	readonly site_name: string;
	readonly title: Option.Option<string>;
}

/** Stores one metadata document until its absolute expiry time. */
export interface RichLinkCacheEntry {
	readonly document: RichLinkMetadataDocument;
	readonly expires_at_ms: number;
}

/** Abstracts rich-link cache storage so persistence can replace memory later. */
export class RichLinkMetadataCache extends Context.Service<
	RichLinkMetadataCache,
	{
		readonly Get: (key: string) => Effect.Effect<Option.Option<RichLinkCacheEntry>>;
		readonly Put: (key: string, entry: RichLinkCacheEntry) => Effect.Effect<void>;
	}
>()("Artisan/RichLinkMetadataCache") {}

/** Reports whether a metadata response was fetched or served from cache. */
export interface RichLinkCacheMetadata {
	readonly expires_at_ms: number;
	readonly status: "hit" | "miss";
}

/** Contains normalized rich-link metadata plus cache provenance. */
export interface RichLinkMetadataResult extends RichLinkMetadataDocument {
	readonly cache: RichLinkCacheMetadata;
}

/** Resolves bounded external HTTP(S) metadata under the rich-link SSRF policy. */
export class RichLinkMetadata extends Context.Service<
	RichLinkMetadata,
	{
		readonly Resolve: (
			url: string,
		) => Effect.Effect<RichLinkMetadataResult, RichLinkMetadataError>;
	}
>()("Artisan/RichLinkMetadata") {}
