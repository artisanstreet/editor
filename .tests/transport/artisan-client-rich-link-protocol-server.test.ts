import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Layer, Option, Stream } from "effect";
import { describe, expect, it } from "vitest";

import {
	make_backend_runtime,
	make_in_memory_rich_link_asset_store_layer,
	make_rich_link_metadata_layer,
	ProtocolServer,
	RichLinkClock,
	RichLinkDnsResolver,
	RichLinkHttpTransport,
	RichLinkMetadata,
	RichLinkTransportError,
	type RichLinkHttpResponse,
} from "@artisan/backend";
import { ArtisanClientError } from "@artisan/transport";
import { BackendBinaryStreamSourceLive, BinaryStreamSource } from "@artisan/transport/server";

import { make_in_memory_rich_link_cache_layer } from "../../modules/backend/src/preview/rich-link-infrastructure";
import {
	make_transport_test_harness_with_protocol_server,
	wait_for,
} from "./message-channel-harness";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const favicon_one = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x01]);
const favicon_two = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x02]);

function response(body: string | Uint8Array, content_type: string): RichLinkHttpResponse {
	return {
		body: typeof body === "string" ? new TextEncoder().encode(body) : body,
		headers: { "content-type": content_type },
		status: 200,
	};
}

function make_rich_links_layer() {
	const routes = new Map<string, RichLinkHttpResponse>([
		[
			"https://links.example/first",
			response(
				`<html><head><title> First link </title><meta property="og:site_name" content="Links"><link rel="icon" href="/first.png"></head></html>`,
				"text/html; charset=utf-8",
			),
		],
		["https://links.example/first.png", response(favicon_one, "image/png")],
		[
			"https://links.example/second",
			response(
				`<html><head><title> Second link </title><link rel="icon" href="/second.png"></head></html>`,
				"text/html; charset=utf-8",
			),
		],
		["https://links.example/second.png", response(favicon_two, "image/png")],
		[
			"https://links.example/controls",
			response(
				'<html><head><title>Alpha\u007fBeta\u0085Gamma</title><meta property="og:site_name" content="Site\u007fName"></head></html>',
				"text/html; charset=utf-8",
			),
		],
	]);
	const dns = Layer.succeed(RichLinkDnsResolver, {
		Resolve: () => Effect.succeed([{ address: "93.184.216.34", family: 4 }]),
	});
	const transport = Layer.succeed(RichLinkHttpTransport, {
		Request: (input) => {
			if (input.url === "https://failure.example/private?token=raw-token") {
				return Effect.fail(
					new RichLinkTransportError({
						cause: new Error("raw transport cause"),
						code: "request",
						url: input.url,
					}),
				);
			}

			const found = routes.get(input.url);

			return found
				? Effect.succeed(found)
				: Effect.succeed({
						body: new Uint8Array(),
						headers: { "content-type": "text/plain" },
						status: 404,
					});
		},
	});
	const infrastructure = Layer.mergeAll(
		dns,
		transport,
		Layer.succeed(RichLinkClock, { Now: Effect.succeed(1_000) }),
		make_in_memory_rich_link_cache_layer(),
	);
	const asset_store = make_in_memory_rich_link_asset_store_layer({
		max_entries: 1,
		max_total_bytes: 128 * 1024,
	});

	return make_rich_link_metadata_layer().pipe(
		Layer.provide(infrastructure),
		Layer.provideMerge(asset_store),
	);
}

function make_invalid_rich_links_layer() {
	return Layer.mergeAll(
		Layer.succeed(RichLinkMetadata, {
			Resolve: (url) =>
				Effect.succeed({
					cache: { expires_at_ms: 2_000, status: "miss" as const },
					favicon: Option.none(),
					fetched_at_ms: 1_000,
					final_url: url,
					page_name: "Unsafe\u007fName",
					requested_url: url,
					site_name: "Links",
					title: Option.none(),
				}),
		}),
		make_in_memory_rich_link_asset_store_layer(),
	);
}

function collect_bytes(chunks: Iterable<Uint8Array>) {
	return Uint8Array.from([...chunks].flatMap((chunk) => [...chunk]));
}

describe("ArtisanClient rich-link metadata and asset streams with the backend ProtocolServer", () => {
	it("rejects invalid service output as a correlated error without reconnecting", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-rich-link-output-guard-"));
		const database_path = join(root, "artisan.db");
		let runtime: ReturnType<typeof make_backend_runtime> | undefined;
		let harness:
			| Awaited<ReturnType<typeof make_transport_test_harness_with_protocol_server>>
			| undefined;

		try {
			runtime = make_backend_runtime({
				database_path,
				migrations_path,
				rich_links: make_invalid_rich_links_layer(),
			});
			const protocol_server = await runtime.runPromise(ProtocolServer);

			harness = await make_transport_test_harness_with_protocol_server(protocol_server, {
				client: { reconnect_delay_ms: 5 },
			});
			const active_harness = harness;
			const failure = await Effect.runPromise(
				active_harness.client
					.GetRichLinkMetadata({ url: "https://links.example/invalid-service" })
					.pipe(
						Effect.timeoutOrElse({
							duration: 2_000,
							orElse: () =>
								Effect.fail(
									new Error(
										"The invalid rich-link result was not rejected in-band.",
									),
								),
						}),
						Effect.flip,
					),
			);
			const snapshot = active_harness.connector_snapshot();

			expect(failure).toBeInstanceOf(ArtisanClientError);
			expect(failure).toMatchObject({
				code: "protocol",
				protocol_code: "rich_link.parse",
				retryable: false,
			});
			expect(snapshot.connections).toBe(1);
			expect(snapshot.rich_link_metadata_query_attempts).toHaveLength(1);
		} finally {
			await harness?.dispose();
			await runtime?.dispose();
			await rm(root, { force: true, recursive: true });
		}
	});

	it("keeps the shared connection healthy across bounded URLs and remote text", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-rich-link-bounds-"));
		const database_path = join(root, "artisan.db");
		const expanding_url = `https://unicode.example/${"\u{1f600}".repeat(200)}`;
		let runtime: ReturnType<typeof make_backend_runtime> | undefined;
		let harness:
			| Awaited<ReturnType<typeof make_transport_test_harness_with_protocol_server>>
			| undefined;

		try {
			runtime = make_backend_runtime({
				database_path,
				migrations_path,
				rich_links: make_rich_links_layer(),
			});
			const protocol_server = await runtime.runPromise(ProtocolServer);

			harness = await make_transport_test_harness_with_protocol_server(protocol_server, {
				client: { reconnect_delay_ms: 5 },
			});
			const active_harness = harness;
			const invalid_url = await Effect.runPromise(
				active_harness.client.GetRichLinkMetadata({ url: expanding_url }).pipe(
					Effect.timeoutOrElse({
						duration: 2_000,
						orElse: () =>
							Effect.fail(
								new Error("The bounded rich-link error was not correlated."),
							),
					}),
					Effect.flip,
				),
			);
			const normalized = await Effect.runPromise(
				active_harness.client.GetRichLinkMetadata({
					url: "https://links.example/controls",
				}),
			);
			const snapshot = active_harness.connector_snapshot();

			expect(expanding_url.length).toBeLessThan(2_048);
			expect(new URL(expanding_url).href.length).toBeGreaterThan(2_048);
			expect(invalid_url).toBeInstanceOf(ArtisanClientError);
			expect(invalid_url).toMatchObject({
				code: "protocol",
				protocol_code: "rich_link.invalid_url",
				retryable: false,
			});
			expect(normalized).toMatchObject({
				page_name: "Alpha Beta Gamma",
				site_name: "Site Name",
				title: "Alpha Beta Gamma",
			});
			expect(snapshot.connections).toBe(1);
			expect(snapshot.rich_link_metadata_query_attempts).toHaveLength(2);
		} finally {
			await harness?.dispose();
			await runtime?.dispose();
			await rm(root, { force: true, recursive: true });
		}
	});

	it("retries the exact metadata query envelope after result loss and reconnect", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-rich-link-reconnect-"));
		const database_path = join(root, "artisan.db");
		let runtime: ReturnType<typeof make_backend_runtime> | undefined;
		let harness:
			| Awaited<ReturnType<typeof make_transport_test_harness_with_protocol_server>>
			| undefined;

		try {
			runtime = make_backend_runtime({
				database_path,
				migrations_path,
				rich_links: make_rich_links_layer(),
			});
			const protocol_server = await runtime.runPromise(ProtocolServer);

			harness = await make_transport_test_harness_with_protocol_server(protocol_server, {
				client: { reconnect_delay_ms: 5 },
				drop_first_rich_link_metadata_result: true,
			});
			const active_harness = harness;
			const metadata = await Effect.runPromise(
				active_harness.client
					.GetRichLinkMetadata({ url: "https://links.example/first" })
					.pipe(
						Effect.timeoutOrElse({
							duration: 2_000,
							orElse: () =>
								Effect.fail(
									new Error(
										"The rich-link metadata query did not complete after reconnect.",
									),
								),
						}),
					),
			);

			await wait_for(
				() =>
					active_harness.connector_snapshot().rich_link_metadata_query_attempts.length ===
					2,
			);
			const snapshot = active_harness.connector_snapshot();

			expect(metadata.page_name).toBe("First link");
			expect(snapshot.connections).toBeGreaterThanOrEqual(2);
			expect(snapshot.dropped_rich_link_metadata_results).toBe(1);
			expect(snapshot.rich_link_metadata_query_attempts).toHaveLength(2);
			expect(snapshot.rich_link_metadata_query_attempts[1]).toEqual(
				snapshot.rich_link_metadata_query_attempts[0],
			);
		} finally {
			await harness?.dispose();
			await runtime?.dispose();
			await rm(root, { force: true, recursive: true });
		}
	});

	it("keeps resolver metadata and isolated asset reads coherent through eviction and restart", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-rich-link-protocol-"));
		const database_path = join(root, "artisan.db");
		let first_runtime: ReturnType<typeof make_backend_runtime> | undefined;
		let first_harness:
			| Awaited<ReturnType<typeof make_transport_test_harness_with_protocol_server>>
			| undefined;
		let second_runtime: ReturnType<typeof make_backend_runtime> | undefined;
		let second_harness:
			| Awaited<ReturnType<typeof make_transport_test_harness_with_protocol_server>>
			| undefined;

		try {
			first_runtime = make_backend_runtime({
				database_path,
				migrations_path,
				rich_links: make_rich_links_layer(),
			});
			const protocol_server = await first_runtime.runPromise(ProtocolServer);
			const binary_stream_source = await first_runtime.runPromise(
				Effect.service(BinaryStreamSource).pipe(
					Effect.provide(BackendBinaryStreamSourceLive),
				),
			);

			first_harness = await make_transport_test_harness_with_protocol_server(
				protocol_server,
				{
					binary_stream_source,
				},
			);
			const active_first_harness = first_harness;

			const metadata = await Effect.runPromise(
				active_first_harness.client
					.GetRichLinkMetadata({ url: "https://links.example/first" })
					.pipe(
						Effect.timeoutOrElse({
							duration: 2_000,
							orElse: () =>
								Effect.fail(
									new Error(
										"The rich-link metadata result was not correlated back to the ArtisanClient request.",
									),
								),
						}),
					),
			);
			const favicon = metadata.favicon;

			expect(metadata).toMatchObject({
				cache: { status: "miss" },
				final_url: "https://links.example/first",
				page_name: "First link",
				site_name: "Links",
				title: "First link",
			});
			expect(favicon).toMatchObject({
				bytes: favicon_one.byteLength,
				content_type: "image/png",
				source: "document_icon",
				source_url: "https://links.example/first.png",
			});
			expect(favicon?.asset_id).toMatch(/^[0-9a-f]{64}$/);
			expect(favicon).not.toHaveProperty("body");

			if (!favicon) {
				throw new Error("Expected the first rich-link favicon metadata");
			}

			const first_asset_id = favicon.asset_id;

			const copied = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const stream = yield* active_first_harness.client.OpenAsset(first_asset_id);

						return collect_bytes(yield* stream.pipe(Stream.runCollect));
					}),
				),
			);

			expect(copied).toEqual(favicon_one);

			const malicious = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const stream = yield* active_first_harness.client.OpenAsset(
							`${first_asset_id}:terminal:unrelated`,
						);

						return yield* stream.pipe(Stream.runCollect);
					}).pipe(Effect.flip),
				),
			);

			expect(malicious).toMatchObject({ code: "stream_closed" });

			const second_metadata = await Effect.runPromise(
				active_first_harness.client.GetRichLinkMetadata({
					url: "https://links.example/second",
				}),
			);
			const second_favicon = second_metadata.favicon;

			if (!second_favicon) {
				throw new Error("Expected the second rich-link favicon metadata");
			}

			const retained_asset_id = second_favicon.asset_id;
			const evicted = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const stream = yield* active_first_harness.client.OpenAsset(first_asset_id);

						return yield* stream.pipe(Stream.runCollect);
					}).pipe(Effect.flip),
				),
			);

			expect(evicted).toMatchObject({ code: "stream_closed" });

			const failed_metadata = await Effect.runPromise(
				active_first_harness.client
					.GetRichLinkMetadata({ url: "https://failure.example/private?token=raw-token" })
					.pipe(Effect.flip),
			);
			const serialized_failure = JSON.stringify(failed_metadata);

			expect(failed_metadata).toBeInstanceOf(ArtisanClientError);
			expect(failed_metadata).toMatchObject({
				code: "protocol",
				protocol_code: "rich_link.transport",
				retryable: true,
			});
			expect(serialized_failure).not.toContain("failure.example");
			expect(serialized_failure).not.toContain("raw-token");
			expect(serialized_failure).not.toContain("raw transport cause");

			await first_harness.dispose();
			await first_runtime.dispose();
			first_harness = undefined;
			first_runtime = undefined;

			second_runtime = make_backend_runtime({
				database_path,
				migrations_path,
				rich_links: make_rich_links_layer(),
			});
			const restarted_protocol_server = await second_runtime.runPromise(ProtocolServer);
			const restarted_binary_stream_source = await second_runtime.runPromise(
				Effect.service(BinaryStreamSource).pipe(
					Effect.provide(BackendBinaryStreamSourceLive),
				),
			);

			second_harness = await make_transport_test_harness_with_protocol_server(
				restarted_protocol_server,
				{ binary_stream_source: restarted_binary_stream_source },
			);
			const active_second_harness = second_harness;
			const restarted = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const stream =
							yield* active_second_harness.client.OpenAsset(retained_asset_id);

						return yield* stream.pipe(Stream.runCollect);
					}).pipe(Effect.flip),
				),
			);

			expect(restarted).toMatchObject({ code: "stream_closed" });
		} finally {
			await second_harness?.dispose();
			await second_runtime?.dispose();
			await first_harness?.dispose();
			await first_runtime?.dispose();
			await rm(root, { force: true, recursive: true });
		}
	});
});
