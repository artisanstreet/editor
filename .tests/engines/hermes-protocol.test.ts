import { describe, expect, it } from "vitest";
import { Effect } from "effect";
import { WebSocketServer } from "ws";

import { ConnectHermesGateway } from "@artisan/engines";

describe("Hermes gateway protocol", () => {
	it("waits for gateway.ready and resolves JSON-RPC responses", async () => {
		const server = new WebSocketServer({ host: "127.0.0.1", port: 0 });
		await new Promise<void>((resolve, reject) => {
			server.once("listening", resolve);
			server.once("error", reject);
		});
		server.on("connection", (socket) => {
			socket.send(
				JSON.stringify({
					jsonrpc: "2.0",
					method: "event",
					params: { payload: { change_events: true }, type: "gateway.ready" },
				}),
			);
			socket.on("message", (data) => {
				const request = JSON.parse(data.toString()) as { id: number; method: string };
				socket.send(
					JSON.stringify({
						id: request.id,
						jsonrpc: "2.0",
						result: { method: request.method, status: "ok" },
					}),
				);
			});
		});
		const address = server.address();
		if (address === null || typeof address === "string") throw new Error("missing test port");
		const client = await Effect.runPromise(
			ConnectHermesGateway(new URL(`ws://127.0.0.1:${address.port}`)),
		);
		await expect(Effect.runPromise(client.Request("session.active_list", {}))).resolves.toEqual(
			{ method: "session.active_list", status: "ok" },
		);
		await Effect.runPromise(client.Close);
		await new Promise<void>((resolve) => server.close(() => resolve()));
	});
});
