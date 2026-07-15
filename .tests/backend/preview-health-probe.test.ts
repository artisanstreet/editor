import { createServer, type RequestListener, type Server } from "node:http";

import { Effect, Fiber, Layer, Option } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	make_node_preview_health_probe_layer,
	NodePreviewHealthDnsResolverLive,
	NodePreviewHealthProbeLive,
	PreviewHealthDnsResolver,
} from "../../modules/backend/src/preview/node-preview-health-probe";
import {
	PreviewHealthProbe,
	type PreviewTargetRecord,
} from "../../modules/backend/src/preview/preview-target";

const servers: Array<Server> = [];

function target(url: string): PreviewTargetRecord {
	return {
		created_at_ms: 0,
		project_id: "project_preview",
		state: "registered",
		target_id: "target_preview",
		updated_at_ms: 0,
		url,
		workspace_id: "workspace_preview",
	};
}

async function listen(handler: RequestListener, hostname = "127.0.0.1") {
	const server = createServer(handler);
	servers.push(server);
	await new Promise<void>((resolve) => server.listen(0, hostname, resolve));
	const address = server.address();

	if (address === null || typeof address === "string") {
		throw new Error("Expected an ephemeral TCP address");
	}

	return { port: address.port, server };
}

function run_probe(
	layer: Layer.Layer<PreviewHealthProbe, never, never>,
	preview_target: PreviewTargetRecord,
) {
	return Effect.runPromise(probe_effect(layer, preview_target));
}

function probe_effect(
	layer: Layer.Layer<PreviewHealthProbe, never, never>,
	preview_target: PreviewTargetRecord,
) {
	return Effect.provide(
		Effect.scoped(
			Effect.gen(function* () {
				const probe = yield* PreviewHealthProbe;

				return yield* probe.Probe(preview_target);
			}),
		),
		layer,
	);
}

afterEach(async () => {
	await Promise.all(
		servers.splice(0).map(
			(server) =>
				new Promise<void>((resolve) => {
					server.close(() => resolve());
				}),
		),
	);
});

describe("Node preview health probe", () => {
	it.each([200, 302, 404])(
		"treats HTTP %s as healthy without following redirects",
		async (status) => {
			let redirect_hits = 0;
			const { port } = await listen((request, response) => {
				if (request.url === "/destination") {
					redirect_hits += 1;
				}

				response.writeHead(
					status,
					status === 302 ? { location: "/destination" } : undefined,
				);
				response.end("ignored response body");
			});

			const result = await run_probe(
				NodePreviewHealthProbeLive,
				target(`http://127.0.0.1:${port}/health`),
			);

			expect(result.status).toBe("healthy");
			expect(Number.isInteger(result.latency_ms)).toBe(true);
			expect(Option.getOrUndefined(result.status_code)).toBe(status);
			expect(redirect_hits).toBe(0);
		},
	);

	it("treats HTTP 5xx as unhealthy with only the status code exposed", async () => {
		const { port } = await listen((_request, response) => {
			response.writeHead(503);
			response.end("private response body");
		});

		const result = await run_probe(
			NodePreviewHealthProbeLive,
			target(`http://127.0.0.1:${port}`),
		);

		expect(result.status).toBe("unhealthy");
		expect(Option.getOrUndefined(result.status_code)).toBe(503);
		expect(Option.getOrUndefined(result.message)).toBe("Preview server returned an error");
	});

	it("works for 127.0.0.1 and preserves the original localhost Host header", async () => {
		let received_host = "";
		let received_method = "";
		const { port } = await listen((request, response) => {
			received_host = request.headers.host ?? "";
			received_method = request.method ?? "";
			response.writeHead(204);
			response.end();
		});

		const result = await run_probe(
			NodePreviewHealthProbeLive,
			target(`http://localhost:${port}/health`),
		);

		expect(result.status).toBe("healthy");
		expect(received_host).toBe(`localhost:${port}`);
		expect(received_method).toBe("HEAD");
	});

	it("connects through the validated DNS address instead of resolving again", async () => {
		let received_host = "";
		const { port } = await listen((request, response) => {
			received_host = request.headers.host ?? "";
			response.writeHead(204);
			response.end();
		}, "127.0.0.2");
		const dns = Layer.succeed(PreviewHealthDnsResolver, {
			Lookup: () => Effect.succeed([{ address: "127.0.0.2", family: 4 }]),
		});
		const layer = make_node_preview_health_probe_layer().pipe(Layer.provide(dns));

		const result = await run_probe(
			layer,
			target(`http://artisan-pinned-address.localhost:${port}/health`),
		);

		expect(result.status).toBe("healthy");
		expect(received_host).toBe(`artisan-pinned-address.localhost:${port}`);
	});

	it.each([
		[
			{ address: "127.0.0.1", family: 4 },
			{ address: "8.8.8.8", family: 4 },
		],
		[
			{ address: "127.0.0.1", family: 4 },
			{ address: "192.168.1.1", family: 4 },
		],
		[
			{ address: "127.0.0.1", family: 6 },
			{ address: "127.0.0.1", family: 4 },
		],
	])("rejects unsafe DNS answers before connecting", async (first, second) => {
		const addresses = [first, second] as ReadonlyArray<{
			readonly address: string;
			readonly family: 4 | 6;
		}>;
		let target_hits = 0;
		const { port } = await listen((_request, response) => {
			target_hits += 1;
			response.end();
		});
		const dns = Layer.succeed(PreviewHealthDnsResolver, {
			Lookup: () => Effect.succeed(addresses),
		});
		const layer = make_node_preview_health_probe_layer().pipe(Layer.provide(dns));

		const exit = await Effect.runPromiseExit(
			probe_effect(layer, target(`http://unsafe.localhost:${port}`)),
		);

		expect(exit._tag).toBe("Failure");
		expect(target_hits).toBe(0);
	});

	it("returns unhealthy for connection failure and timeout", async () => {
		const refused = await run_probe(NodePreviewHealthProbeLive, target("http://127.0.0.1:1"));
		expect(refused.status).toBe("unhealthy");
		expect(Option.isNone(refused.status_code)).toBe(true);

		const { port } = await listen((_request, _response) => undefined);
		const layer = make_node_preview_health_probe_layer({ timeout_ms: 25 }).pipe(
			Layer.provide(NodePreviewHealthDnsResolverLive),
		);
		const timed_out = await run_probe(layer, target(`http://127.0.0.1:${port}`));

		expect(timed_out.status).toBe("unhealthy");
		expect(Option.isNone(timed_out.status_code)).toBe(true);
	});

	it.each([0, -1, 60_001, Number.MAX_SAFE_INTEGER + 1])(
		"rejects invalid timeout %s with a typed error",
		async (timeout_ms) => {
			const exit = await Effect.runPromiseExit(
				probe_effect(
					make_node_preview_health_probe_layer({ timeout_ms }).pipe(
						Layer.provide(NodePreviewHealthDnsResolverLive),
					),
					target("http://127.0.0.1:1"),
				),
			);

			expect(exit._tag).toBe("Failure");
			if (exit._tag === "Failure") {
				expect(exit.cause.toString()).toContain("PreviewHealthProbeError");
			}
		},
	);

	it("rejects credentials and non-loopback stored targets with a typed error", async () => {
		for (const url of ["http://user:pass@localhost:1", "http://example.com"]) {
			const exit = await Effect.runPromiseExit(
				probe_effect(NodePreviewHealthProbeLive, target(url)),
			);

			expect(exit._tag).toBe("Failure");
			if (exit._tag === "Failure") {
				expect(exit.cause.toString()).toContain("PreviewHealthProbeError");
			}
		}
	});

	it("keeps a cancellation-owned probe scoped", async () => {
		let mark_request_started!: () => void;
		const request_started = new Promise<void>((resolve) => {
			mark_request_started = resolve;
		});
		const { port, server } = await listen((_request, _response) => {
			mark_request_started();
		});
		const fiber = Effect.runFork(
			Effect.scoped(
				Effect.gen(function* () {
					const probe = yield* PreviewHealthProbe;
					yield* probe.Probe(target(`http://127.0.0.1:${port}`));
				}).pipe(Effect.provide(NodePreviewHealthProbeLive)),
			),
		);

		await request_started;
		await Effect.runPromise(Fiber.interrupt(fiber));
		servers.splice(servers.indexOf(server), 1);
		await new Promise<void>((resolve) => server.close(() => resolve()));
		expect(server.listening).toBe(false);
	});
});
