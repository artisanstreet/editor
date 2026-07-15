import { Schema } from "effect";

import ipaddr from "ipaddr.js";

const has_control_character = (value: string) =>
	[...value].some((character) => {
		const code = character.codePointAt(0)!;

		return code <= 31 || (code >= 127 && code <= 159);
	});

const RichLinkTimestampMs = Schema.Int.check(
	Schema.isGreaterThanOrEqualTo(0),
	Schema.isLessThanOrEqualTo(4_102_444_800_000),
);

const is_external_hostname = (hostname: string) => {
	const normalized = hostname
		.toLowerCase()
		.replace(/^\[|\]$/g, "")
		.replace(/\.$/, "");

	if (
		normalized.length === 0 ||
		normalized === "localhost" ||
		normalized.endsWith(".localhost")
	) {
		return false;
	}

	if (!ipaddr.isValid(normalized)) {
		return true;
	}

	const parsed = ipaddr.parse(normalized);

	if (
		parsed.kind() === "ipv6" &&
		"isIPv4MappedAddress" in parsed &&
		parsed.isIPv4MappedAddress()
	) {
		return false;
	}

	return parsed.range() === "unicast";
};

const RichLinkUrl = Schema.String.check(
	Schema.makeFilter<string>((value) => {
		if (value.length > 2048) {
			return "Expected a bounded rich-link URL";
		}

		if (has_control_character(value)) {
			return "Expected a rich-link URL without control characters";
		}

		if (!URL.canParse(value)) {
			return "Expected a valid rich-link URL";
		}

		const url = new URL(value);

		return url.protocol !== "http:" && url.protocol !== "https:"
			? "Expected an HTTP(S) rich-link URL"
			: url.username || url.password
				? "Rich-link URLs must not contain credentials"
				: !is_external_hostname(url.hostname)
					? "Rich-link URLs must target an external host"
					: undefined;
	}),
);

const RichLinkName = Schema.String.check(
	Schema.makeFilter<string>((value) =>
		value.length > 0 && [...value].length <= 512 && !has_control_character(value)
			? undefined
			: "Expected a bounded rich-link name without control characters",
	),
);

const RichLinkAssetId = Schema.String.check(
	Schema.isPattern(/^[0-9a-f]{64}$/, {
		message: "Expected a lowercase SHA-256 asset id",
	}),
);

const RichLinkContentType = Schema.Literals([
	"image/gif",
	"image/ico",
	"image/jpeg",
	"image/png",
	"image/vnd.microsoft.icon",
	"image/webp",
	"image/x-icon",
]);

/** Queries bounded metadata for one credential-free external HTTP(S) URL. */
export const RichLinkMetadataQuery = Schema.Struct({ url: RichLinkUrl });

export type RichLinkMetadataQuery = typeof RichLinkMetadataQuery.Type;

/** Identifies a retained favicon asset that can be read from the binary asset stream. */
export const RichLinkFavicon = Schema.Struct({
	asset_id: RichLinkAssetId,
	bytes: Schema.Int.check(Schema.isGreaterThan(0), Schema.isLessThanOrEqualTo(128 * 1024)),
	content_type: RichLinkContentType,
	source: Schema.Literals(["apple_touch", "document_icon", "fallback"]),
	source_url: RichLinkUrl,
});

export type RichLinkFavicon = typeof RichLinkFavicon.Type;

/** Describes whether the returned metadata document was fetched or served from cache. */
export const RichLinkCacheMetadata = Schema.Struct({
	expires_at_ms: RichLinkTimestampMs,
	status: Schema.Literals(["hit", "miss"]),
});

export type RichLinkCacheMetadata = typeof RichLinkCacheMetadata.Type;

/** Returns normalized metadata and optional favicon references without fetch internals. */
export const RichLinkMetadataQueryResult = Schema.Struct({
	cache: RichLinkCacheMetadata,
	favicon: Schema.optional(RichLinkFavicon),
	fetched_at_ms: RichLinkTimestampMs,
	final_url: RichLinkUrl,
	page_name: RichLinkName,
	requested_url: RichLinkUrl,
	site_name: RichLinkName,
	title: Schema.optional(RichLinkName),
}).check(
	Schema.makeFilter<typeof RichLinkMetadataQueryResult.Type>((value) =>
		value.cache.expires_at_ms >= value.fetched_at_ms
			? undefined
			: "Expected cache expiry at or after the fetch timestamp",
	),
);

export type RichLinkMetadataQueryResult = typeof RichLinkMetadataQueryResult.Type;
