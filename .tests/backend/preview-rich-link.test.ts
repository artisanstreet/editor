import { Context, Deferred, Effect, Layer, Option, Scope } from "effect";
import { describe, expect, it } from "vitest";

import {
	RichLinkAssetStore,
	make_in_memory_rich_link_asset_store_layer,
	type RichLinkAsset,
	type RichLinkAssetStoreOptions,
} from "../../modules/backend/src/preview/rich-link-asset-store";
import {
	RichLinkClock,
	RichLinkDnsResolver,
	RichLinkHttpTransport,
	RichLinkMetadata,
	RichLinkMetadataCache,
	type RichLinkHttpRequest,
	type RichLinkHttpResponse,
	type RichLinkMetadataError,
	type RichLinkMetadataResult,
	type RichLinkResolvedAddress,
} from "../../modules/backend/src/preview/rich-link-metadata";
import {
	make_in_memory_rich_link_cache_layer,
	type InMemoryRichLinkCacheOptions,
} from "../../modules/backend/src/preview/rich-link-infrastructure";
import {
	make_node_rich_link_metadata_layer,
	make_rich_link_metadata_layer,
	type RichLinkMetadataOptions,
} from "../../modules/backend/src/preview/rich-link-service";

interface RichLinkTestHarness {
	readonly active_requests: () => number;
	readonly advance: (milliseconds: number) => void;
	readonly dns_calls: Array<string>;
	readonly get_asset: (asset_id: string) => Promise<Option.Option<RichLinkAsset>>;
	readonly requests: Array<RichLinkHttpRequest>;
	readonly resolve: (url: string) => Promise<RichLinkMetadataResult>;
	readonly resolve_image: (url: string) => Promise<{
		readonly asset_id: string;
		readonly bytes: number;
		readonly content_type: string;
	}>;
	readonly resolve_with_signal: (
		url: string,
		signal: AbortSignal,
	) => Promise<RichLinkMetadataResult>;
}

async function resolve_error(harness: RichLinkTestHarness, url: string) {
	try {
		await harness.resolve(url);
	} catch (error) {
		return error as RichLinkMetadataError;
	}

	throw new Error("expected rich-link resolution to fail");
}

interface RichLinkTestOptions {
	readonly addresses?: Readonly<Record<string, ReadonlyArray<RichLinkResolvedAddress>>>;
	readonly asset_store?: RichLinkAssetStoreOptions;
	readonly hang_urls?: ReadonlySet<string>;
	readonly limits?: RichLinkMetadataOptions;
	readonly metadata_cache?: InMemoryRichLinkCacheOptions;
	readonly request_gates?: ReadonlyMap<string, Deferred.Deferred<void>>;
	readonly request_started?: ReadonlyMap<string, Deferred.Deferred<void>>;
	readonly routes: ReadonlyMap<string, RichLinkHttpResponse>;
}

const png_bytes = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x01]);
const ico_bytes = new Uint8Array([0x00, 0x00, 0x01, 0x00, 0x01, 0x00]);
const gif_bytes = new Uint8Array([0x47, 0x49, 0x46, 0x38, 0x39, 0x61]);
const jpeg_bytes = new Uint8Array([0xff, 0xd8, 0xff]);
const webp_bytes = new Uint8Array([
	0x52, 0x49, 0x46, 0x46, 0x04, 0x00, 0x00, 0x00, 0x57, 0x45, 0x42, 0x50,
]);

function response(
	status: number,
	body: string | Uint8Array,
	headers: Readonly<Record<string, string>> = {},
): RichLinkHttpResponse {
	return {
		body: typeof body === "string" ? new TextEncoder().encode(body) : body,
		headers,
		status,
	};
}

async function make_harness(options: RichLinkTestOptions): Promise<RichLinkTestHarness> {
	const requests: Array<RichLinkHttpRequest> = [];
	const dns_calls: Array<string> = [];
	let active_requests = 0;
	let now_ms = 1_000;
	const default_address: RichLinkResolvedAddress = {
		address: "93.184.216.34",
		family: 4,
	};
	const dns = Layer.succeed(RichLinkDnsResolver, {
		Resolve: (hostname) => {
			dns_calls.push(hostname);

			return Effect.succeed(options.addresses?.[hostname] ?? [default_address]);
		},
	});
	const transport = Layer.succeed(RichLinkHttpTransport, {
		Request: (input) =>
			Effect.acquireUseRelease(
				Effect.gen(function* () {
					active_requests += 1;
					requests.push(input);
					const started = options.request_started?.get(input.url);

					if (started !== undefined) {
						yield* Deferred.succeed(started, undefined);
					}
				}),
				() => {
					if (options.hang_urls?.has(input.url)) {
						return Effect.never;
					}

					const resolved =
						options.routes.get(input.url) ??
						response(404, "missing", { "content-type": "text/plain" });
					const gate = options.request_gates?.get(input.url);

					return gate === undefined
						? Effect.succeed(resolved)
						: Deferred.await(gate).pipe(Effect.as(resolved));
				},
				() =>
					Effect.sync(() => {
						active_requests -= 1;
					}),
			),
	});
	const clock = Layer.succeed(RichLinkClock, {
		Now: Effect.sync(() => now_ms),
	});
	const infrastructure = Layer.mergeAll(
		dns,
		transport,
		clock,
		make_in_memory_rich_link_cache_layer(options.metadata_cache),
	);
	const asset_store_layer = make_in_memory_rich_link_asset_store_layer(options.asset_store);
	const layer = make_rich_link_metadata_layer(options.limits).pipe(
		Layer.provide(infrastructure),
		Layer.provideMerge(asset_store_layer),
	);
	const scope = await Effect.runPromise(Scope.make());
	const context = await Effect.runPromise(Layer.buildWithScope(layer, scope));
	const services = {
		asset_store: Context.get(context, RichLinkAssetStore),
		metadata: Context.get(context, RichLinkMetadata),
	};

	return {
		active_requests: () => active_requests,
		advance: (milliseconds) => {
			now_ms += milliseconds;
		},
		dns_calls,
		get_asset: (asset_id) => Effect.runPromise(services.asset_store.Get(asset_id)),
		requests,
		resolve: (url) => Effect.runPromise(services.metadata.Resolve(url)),
		resolve_image: (url) => Effect.runPromise(services.metadata.ResolveImage(url)),
		resolve_with_signal: (url, signal) =>
			Effect.runPromise(services.metadata.Resolve(url), { signal }),
	};
}

async function make_asset_store(options: RichLinkAssetStoreOptions) {
	return Effect.runPromise(
		Effect.service(RichLinkAssetStore).pipe(
			Effect.provide(make_in_memory_rich_link_asset_store_layer(options)),
		),
	);
}

describe("RichLinkMetadata", () => {
	it("retains a bounded, signature-verified image through the safe fetch boundary", async () => {
		const url = "https://avatars.example.com/project.png";
		const harness = await make_harness({
			routes: new Map([[url, response(200, png_bytes, { "content-type": "image/png" })]]),
		});

		const image = await harness.resolve_image(url);
		expect(image).toMatchObject({ bytes: png_bytes.byteLength, content_type: "image/png" });
		const retained = await harness.get_asset(image.asset_id);
		expect(Option.getOrUndefined(retained)?.body).toEqual(png_bytes);
		expect(harness.requests[0]?.accept).toBe("image/*");
	});

	it("rejects image responses whose bytes do not match their declared media type", async () => {
		const url = "https://avatars.example.com/not-really.png";
		const harness = await make_harness({
			routes: new Map([
				[url, response(200, "not an image", { "content-type": "image/png" })],
			]),
		});

		await expect(harness.resolve_image(url)).rejects.toMatchObject({
			code: "content_type",
			url,
		});
	});

	it("applies title, Open Graph, site, and favicon precedence after redirects", async () => {
		const routes = new Map<string, RichLinkHttpResponse>([
			["https://example.com/start", response(302, "", { location: "/articles/final" })],
			[
				"https://example.com/articles/final",
				response(
					200,
					`<html><head>
						<title>  Document Title  </title>
						<meta property="og:title" content="Open Graph Name">
						<meta name="twitter:title" content="Twitter Name">
						<meta property="og:site_name" content="Example Publication">
						<meta name="application-name" content="Fallback App">
						<link rel="apple-touch-icon" href="/apple.png">
						<link rel="icon" href="../assets/icon.png">
					</head></html>`,
					{ "content-type": "text/html; charset=utf-8" },
				),
			],
			[
				"https://example.com/assets/icon.png",
				response(200, png_bytes, { "content-type": "image/png" }),
			],
		]);
		const harness = await make_harness({ routes });

		const result = await harness.resolve("https://example.com/start");
		const favicon = Option.getOrThrow(result.favicon);

		expect(result.final_url).toBe("https://example.com/articles/final");
		expect(Option.getOrThrow(result.title)).toBe("Document Title");
		expect(result.page_name).toBe("Open Graph Name");
		expect(result.site_name).toBe("Example Publication");
		expect(favicon).toMatchObject({
			bytes: png_bytes.byteLength,
			content_type: "image/png",
			source: "document_icon",
			source_url: "https://example.com/assets/icon.png",
		});
		expect(favicon.asset_id).toMatch(/^[0-9a-f]{64}$/);
		const stored = Option.getOrThrow(await harness.get_asset(favicon.asset_id));

		expect(stored.body).toEqual(png_bytes);
		expect(harness.requests.map((request) => request.url)).toEqual([
			"https://example.com/start",
			"https://example.com/articles/final",
			"https://example.com/assets/icon.png",
		]);
		expect(harness.dns_calls).toEqual(["example.com", "example.com", "example.com"]);
		expect(harness.requests.every((request) => request.host_header === "example.com")).toBe(
			true,
		);
		expect(
			harness.requests.every(
				(request) =>
					request.pinned_address.address === "93.184.216.34" &&
					request.tls_server_name === "example.com",
			),
		).toBe(true);
		expect(harness.active_requests()).toBe(0);
	});

	it("recovers malformed HTML and falls back to origin favicon and names", async () => {
		const routes = new Map<string, RichLinkHttpResponse>([
			[
				"https://fallback.example/path",
				response(200, "<html><head><meta broken", { "content-type": "text/html" }),
			],
			[
				"https://fallback.example/favicon.ico",
				response(200, ico_bytes, {
					"content-type": "image/vnd.microsoft.icon",
				}),
			],
		]);
		const harness = await make_harness({ routes });

		const result = await harness.resolve("https://fallback.example/path");

		expect(Option.isNone(result.title)).toBe(true);
		expect(result.page_name).toBe("fallback.example");
		expect(result.site_name).toBe("fallback.example");
		expect(Option.getOrThrow(result.favicon).source).toBe("fallback");
		expect(harness.active_requests()).toBe(0);
	});

	it.each([
		["image/png", png_bytes],
		["image/gif", gif_bytes],
		["image/jpeg", jpeg_bytes],
		["image/vnd.microsoft.icon", ico_bytes],
		["image/webp", webp_bytes],
	] as const)("accepts verified %s favicon bytes", async (content_type, body) => {
		const routes = new Map<string, RichLinkHttpResponse>([
			[
				"https://magic.example/",
				response(200, '<link rel="icon" href="/icon">', {
					"content-type": "text/html",
				}),
			],
			["https://magic.example/icon", response(200, body, { "content-type": content_type })],
		]);
		const harness = await make_harness({ routes });

		const result = await harness.resolve("https://magic.example/");
		const favicon = Option.getOrThrow(result.favicon);
		const stored = Option.getOrThrow(await harness.get_asset(favicon.asset_id));

		expect(favicon.content_type).toBe(content_type);
		expect(stored.body).toEqual(body);
	});

	it.each([
		["image/png", new TextEncoder().encode("<html>not an image</html>")],
		["image/svg+xml", new TextEncoder().encode("<svg><script /></svg>")],
	] as const)(
		"rejects unsafe or mismatched %s bytes and attempts the origin fallback",
		async (content_type, body) => {
			const routes = new Map<string, RichLinkHttpResponse>([
				[
					"https://unsafe-icon.example/",
					response(200, '<link rel="icon" href="/claimed">', {
						"content-type": "text/html",
					}),
				],
				[
					"https://unsafe-icon.example/claimed",
					response(200, body, { "content-type": content_type }),
				],
				[
					"https://unsafe-icon.example/favicon.ico",
					response(200, ico_bytes, { "content-type": "image/x-icon" }),
				],
			]);
			const harness = await make_harness({ routes });

			const result = await harness.resolve("https://unsafe-icon.example/");
			const favicon = Option.getOrThrow(result.favicon);
			const stored = Option.getOrThrow(await harness.get_asset(favicon.asset_id));

			expect(favicon.source).toBe("fallback");
			expect(favicon.source_url).toBe("https://unsafe-icon.example/favicon.ico");
			expect(stored.body).toEqual(ico_bytes);
			expect(harness.requests.map((request) => request.url)).toEqual([
				"https://unsafe-icon.example/",
				"https://unsafe-icon.example/claimed",
				"https://unsafe-icon.example/favicon.ico",
			]);
		},
	);

	it("revalidates every redirect before opening the next socket", async () => {
		const routes = new Map<string, RichLinkHttpResponse>([
			[
				"https://public.example/start",
				response(302, "", { location: "http://127.0.0.1:4173/private" }),
			],
		]);
		const harness = await make_harness({ routes });

		const error = await resolve_error(harness, "https://public.example/start");

		expect(error).toMatchObject({ code: "blocked_address" });
		expect(harness.requests).toHaveLength(1);
		expect(harness.active_requests()).toBe(0);
	});

	it.each([
		"http://127.0.0.1/",
		"http://10.0.0.1/",
		"http://169.254.1.1/",
		"http://192.0.2.1/",
		"http://224.0.0.1/",
		"http://[::1]/",
		"http://[fe80::1]/",
		"http://[fc00::1]/",
		"http://[ff02::1]/",
		"http://[::ffff:127.0.0.1]/",
	])("rejects non-public literal address %s", async (url) => {
		const harness = await make_harness({ routes: new Map() });

		const error = await resolve_error(harness, url);

		expect(error).toMatchObject({ code: "blocked_address" });
		expect(harness.requests).toHaveLength(0);
	});

	it.each([
		["ftp://example.com/file", "invalid_url"],
		["https://user:password@example.com/", "invalid_url"],
		["http://localhost:4173/", "blocked_address"],
	] as const)("rejects unsafe URL boundary %s", async (url, code) => {
		const harness = await make_harness({ routes: new Map() });

		const error = await resolve_error(harness, url);

		expect(error.code).toBe(code);
		expect(harness.requests).toHaveLength(0);
	});

	it("rejects mixed public and private DNS answers before transport", async () => {
		const harness = await make_harness({
			addresses: {
				"mixed.example": [
					{ address: "93.184.216.34", family: 4 },
					{ address: "10.0.0.4", family: 4 },
				],
			},
			routes: new Map(),
		});

		const error = await resolve_error(harness, "https://mixed.example/");

		expect(error).toMatchObject({ code: "blocked_address" });
		expect(harness.requests).toHaveLength(0);
	});

	it("rejects DNS address family mismatches before transport", async () => {
		const harness = await make_harness({
			addresses: {
				"family.example": [{ address: "93.184.216.34", family: 6 }],
			},
			routes: new Map(),
		});

		const error = await resolve_error(harness, "https://family.example/");

		expect(error).toMatchObject({ code: "blocked_address" });
		expect(harness.requests).toHaveLength(0);
	});

	it("enforces HTML size, content type, redirect, and timeout limits", async () => {
		const oversized = await make_harness({
			limits: { max_html_bytes: 4 },
			routes: new Map([
				["https://large.example/", response(200, "12345", { "content-type": "text/html" })],
			]),
		});
		const wrong_type = await make_harness({
			routes: new Map([
				[
					"https://json.example/",
					response(200, "{}", { "content-type": "application/json" }),
				],
			]),
		});
		const redirects = await make_harness({
			limits: { max_redirects: 1 },
			routes: new Map([
				["https://redirect.example/one", response(302, "", { location: "/two" })],
				["https://redirect.example/two", response(302, "", { location: "/three" })],
			]),
		});
		const timeout = await make_harness({
			hang_urls: new Set(["https://timeout.example/"]),
			limits: { connect_timeout_ms: 10, response_timeout_ms: 10 },
			routes: new Map(),
		});

		const size_error = await resolve_error(oversized, "https://large.example/");
		const type_error = await resolve_error(wrong_type, "https://json.example/");
		const redirect_error = await resolve_error(redirects, "https://redirect.example/one");
		const timeout_error = await resolve_error(timeout, "https://timeout.example/");

		expect(size_error).toMatchObject({ code: "response_size" });
		expect(type_error).toMatchObject({ code: "content_type" });
		expect(redirect_error).toMatchObject({ code: "redirect" });
		expect(timeout_error).toMatchObject({ code: "timeout" });
		expect(timeout.active_requests()).toBe(0);
	});

	it("bounds favicon responses without failing page metadata", async () => {
		const routes = new Map<string, RichLinkHttpResponse>([
			[
				"https://icons.example/",
				response(200, '<title>Page</title><link rel="icon" href="/large.png">', {
					"content-type": "text/html",
				}),
			],
			[
				"https://icons.example/large.png",
				response(200, new Uint8Array([1, 2, 3]), { "content-type": "image/png" }),
			],
		]);
		const harness = await make_harness({
			limits: { max_favicon_bytes: 2 },
			routes,
		});

		const result = await harness.resolve("https://icons.example/");

		expect(Option.isNone(result.favicon)).toBe(true);
		expect(
			harness.requests
				.filter((request) => request.accept === "image/*")
				.every((request) => request.max_bytes === 2),
		).toBe(true);
	});

	it("bounds document favicon candidate attempts before fallback", async () => {
		const routes = new Map<string, RichLinkHttpResponse>([
			[
				"https://candidate.example/",
				response(
					200,
					'<link rel="icon" href="/first.png"><link rel="icon" href="/second.png">',
					{
						"content-type": "text/html",
					},
				),
			],
			[
				"https://candidate.example/second.png",
				response(200, new Uint8Array([1]), { "content-type": "image/png" }),
			],
		]);
		const harness = await make_harness({
			limits: { max_favicon_candidates: 1 },
			routes,
		});

		const result = await harness.resolve("https://candidate.example/");

		expect(Option.isNone(result.favicon)).toBe(true);
		expect(harness.requests.map((request) => request.url)).not.toContain(
			"https://candidate.example/second.png",
		);
	});

	it("serves fresh cache entries and refreshes after TTL expiry", async () => {
		const routes = new Map<string, RichLinkHttpResponse>([
			[
				"https://cache.example/",
				response(200, '<title>Cached</title><link rel="icon" href="/icon.png">', {
					"content-type": "text/html",
				}),
			],
			[
				"https://cache.example/icon.png",
				response(200, png_bytes, { "content-type": "image/png" }),
			],
		]);
		const harness = await make_harness({ limits: { cache_ttl_ms: 100 }, routes });

		const first = await harness.resolve("https://cache.example/#section");
		const first_favicon = Option.getOrThrow(first.favicon);
		const stored_before_hit = Option.getOrThrow(
			await harness.get_asset(first_favicon.asset_id),
		);
		const second = await harness.resolve("https://cache.example/");
		const second_favicon = Option.getOrThrow(second.favicon);
		const stored_after_hit = Option.getOrThrow(
			await harness.get_asset(second_favicon.asset_id),
		);

		expect(first.cache.status).toBe("miss");
		expect(second.cache.status).toBe("hit");
		expect(second_favicon.asset_id).toBe(first_favicon.asset_id);
		expect(stored_before_hit.body).toEqual(png_bytes);
		expect(stored_after_hit.body).toEqual(png_bytes);
		expect(harness.requests).toHaveLength(2);

		harness.advance(101);

		const third = await harness.resolve("https://cache.example/");

		expect(third.cache.status).toBe("miss");
		expect(harness.requests).toHaveLength(4);
		expect(harness.active_requests()).toBe(0);
	});

	it("coalesces overlapping canonical cold resolutions without retaining the result", async () => {
		const url = "https://single-flight.example/";
		const gate = await Effect.runPromise(Deferred.make<void>());
		const started = await Effect.runPromise(Deferred.make<void>());
		const harness = await make_harness({
			request_gates: new Map([[url, gate]]),
			request_started: new Map([[url, started]]),
			routes: new Map([
				[
					url,
					response(200, "<title>Single flight</title>", {
						"content-type": "text/html",
					}),
				],
				[
					"https://single-flight.example/favicon.ico",
					response(404, "missing", { "content-type": "text/plain" }),
				],
			]),
		});

		const first = harness.resolve(`${url}#first`);

		await Effect.runPromise(Deferred.await(started));

		const second = harness.resolve(url);

		expect(harness.requests).toHaveLength(1);
		await Effect.runPromise(Deferred.succeed(gate, undefined));

		const [first_result, second_result] = await Promise.all([first, second]);

		expect(first_result.cache.status).toBe("miss");
		expect(second_result.cache.status).toBe("miss");
		expect(harness.requests.map((request) => request.url)).toEqual([
			url,
			"https://single-flight.example/favicon.ico",
		]);
		expect(harness.active_requests()).toBe(0);

		const refreshed = await harness.resolve(url);

		expect(refreshed.cache.status).toBe("hit");
	});

	it("keeps joined cold resolution alive when the initiating route is interrupted", async () => {
		const url = "https://interrupted-flight.example/";
		const gate = await Effect.runPromise(Deferred.make<void>());
		const started = await Effect.runPromise(Deferred.make<void>());
		const harness = await make_harness({
			request_gates: new Map([[url, gate]]),
			request_started: new Map([[url, started]]),
			routes: new Map([
				[
					url,
					response(200, "<title>Shared flight</title>", { "content-type": "text/html" }),
				],
				[
					"https://interrupted-flight.example/favicon.ico",
					response(404, "missing", { "content-type": "text/plain" }),
				],
			]),
		});
		const controller = new AbortController();
		const first = harness.resolve_with_signal(`${url}#route`, controller.signal);

		await Effect.runPromise(Deferred.await(started));

		const second = harness.resolve(url);
		controller.abort();

		await expect(first).rejects.toThrow("interrupted");
		await Effect.runPromise(Deferred.succeed(gate, undefined));

		await expect(second).resolves.toMatchObject({
			cache: { status: "miss" },
			final_url: url,
		});
		expect(harness.requests.map((request) => request.url)).toEqual([
			url,
			"https://interrupted-flight.example/favicon.ico",
		]);
		expect(harness.active_requests()).toBe(0);
	});

	it("does not retain a failed cold resolution for a later request", async () => {
		const url = "https://retry-flight.example/";
		const routes = new Map<string, RichLinkHttpResponse>();
		const harness = await make_harness({ routes });

		await expect(harness.resolve(url)).rejects.toMatchObject({ code: "status" });
		routes.set(url, response(200, "<title>Retry</title>", { "content-type": "text/html" }));
		routes.set(
			"https://retry-flight.example/favicon.ico",
			response(404, "missing", { "content-type": "text/plain" }),
		);

		await expect(harness.resolve(url)).resolves.toMatchObject({
			cache: { status: "miss" },
			final_url: url,
		});
		expect(harness.requests.map((request) => request.url)).toEqual([
			url,
			url,
			"https://retry-flight.example/favicon.ico",
		]);
	});

	it("evicts retained assets under both byte and entry limits", async () => {
		const byte_bounded = await make_asset_store({ max_entries: 4, max_total_bytes: 4 });
		const first = await Effect.runPromise(
			byte_bounded.Put({ body: new Uint8Array([1, 2, 3]), content_type: "image/png" }),
		);
		const second = await Effect.runPromise(
			byte_bounded.Put({ body: new Uint8Array([4, 5, 6]), content_type: "image/png" }),
		);

		expect(Option.isNone(await Effect.runPromise(byte_bounded.Get(first.asset_id)))).toBe(true);
		expect(Option.isSome(await Effect.runPromise(byte_bounded.Get(second.asset_id)))).toBe(
			true,
		);

		const entry_bounded = await make_asset_store({ max_entries: 1, max_total_bytes: 32 });
		const third = await Effect.runPromise(
			entry_bounded.Put({ body: new Uint8Array([7]), content_type: "image/png" }),
		);
		const fourth = await Effect.runPromise(
			entry_bounded.Put({ body: new Uint8Array([8]), content_type: "image/png" }),
		);

		expect(Option.isNone(await Effect.runPromise(entry_bounded.Get(third.asset_id)))).toBe(
			true,
		);
		expect(Option.isSome(await Effect.runPromise(entry_bounded.Get(fourth.asset_id)))).toBe(
			true,
		);
	});

	it("validates resolver, metadata-cache, and asset-store limits during layer build", async () => {
		await expect(
			make_harness({ limits: { max_html_bytes: 0 }, routes: new Map() }),
		).rejects.toMatchObject({ code: "configuration" });
		await expect(
			make_harness({
				asset_store: { max_entries: 1, max_total_bytes: 8 },
				limits: { max_favicon_bytes: 9 },
				routes: new Map(),
			}),
		).rejects.toMatchObject({ code: "configuration" });

		const cache_error = await Effect.runPromise(
			Effect.service(RichLinkMetadataCache).pipe(
				Effect.provide(make_in_memory_rich_link_cache_layer({ max_entries: 0 })),
				Effect.flip,
			),
		);
		const asset_error = await Effect.runPromise(
			Effect.service(RichLinkAssetStore).pipe(
				Effect.provide(
					make_in_memory_rich_link_asset_store_layer({
						max_entries: 1,
						max_total_bytes: 0,
					}),
				),
				Effect.flip,
			),
		);

		expect(cache_error).toMatchObject({ code: "configuration" });
		expect(asset_error).toMatchObject({ code: "configuration" });
	});

	it("exposes the asset read service from production composition without fetching", async () => {
		const result = await Effect.runPromise(
			Effect.gen(function* () {
				yield* RichLinkMetadata;
				const asset_store = yield* RichLinkAssetStore;

				return yield* asset_store.Get("missing");
			}).pipe(Effect.provide(make_node_rich_link_metadata_layer())),
		);

		expect(Option.isNone(result)).toBe(true);
	});
});
