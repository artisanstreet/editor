import { createServer, type Server } from "node:http";

import { Effect } from "effect";
import { afterAll, beforeAll, describe, expect, it } from "vitest";

import { NodeRichLinkHttpTransportLive } from "../../modules/backend/src/preview/node-rich-link-transport";
import { RichLinkHttpTransport } from "../../modules/backend/src/preview/rich-link-metadata";

let server: Server;
let port = 0;

beforeAll(async () => {
	server = createServer((request, response) => {
		response.on("error", () => undefined);

		if (request.url === "/slow") {
			setTimeout(() => {
				response.writeHead(200, { "content-type": "text/plain" });
				response.end("late");
			}, 100);
			return;
		}

		response.writeHead(200, { "content-type": "text/plain" });
		response.end("artisan");
	});
	await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
	const address = server.address();

	if (address === null || typeof address === "string") {
		throw new Error("expected a TCP test server address");
	}

	port = address.port;
});

afterAll(async () => {
	await new Promise<void>((resolve, reject) =>
		server.close((cause) => (cause === undefined ? resolve() : reject(cause))),
	);
});

const Request = (path: string, response_timeout_ms: number) =>
	Effect.gen(function* () {
		const transport = yield* RichLinkHttpTransport;

		return yield* transport.Request({
			accept: "text/plain",
			connect_timeout_ms: 1_000,
			host_header: `localhost:${port}`,
			max_bytes: 128,
			pinned_address: { address: "127.0.0.1", family: 4 },
			response_timeout_ms,
			tls_server_name: "localhost",
			url: `http://localhost:${port}${path}`,
		});
	}).pipe(Effect.provide(NodeRichLinkHttpTransportLive));

describe("NodeRichLinkHttpTransport", () => {
	it("reads a bounded response through the pinned address", async () => {
		const response = await Effect.runPromise(Request("/", 1_000));

		expect(response.status).toBe(200);
		expect(Buffer.from(response.body).toString("utf8")).toBe("artisan");
		expect(response.headers["content-type"]).toBe("text/plain");
	});

	it("interrupts the Node request when the Effect response timeout expires", async () => {
		const error = await Effect.runPromise(Request("/slow", 10).pipe(Effect.flip));

		expect(error.code).toBe("response_timeout");
	});
});
