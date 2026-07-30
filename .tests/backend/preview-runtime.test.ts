import { createServer, type IncomingMessage, type ServerResponse } from "node:http";

import { Effect, Layer, Option } from "effect";
import { ChildProcessSpawner } from "effect/unstable/process";
import { afterEach, describe, expect, it } from "vitest";

import { make_node_preview_health_probe_layer } from "../../modules/backend/src/preview/node-preview-health-probe";
import {
	PreviewHealthProbe,
	PreviewTarget,
	PreviewTargetClock,
	type PreviewTargetRecord,
} from "../../modules/backend/src/preview/target";
import { make_preview_target_layer } from "../../modules/backend/src/preview/target-service";
import {
	make_preview_inspection_layer,
	make_preview_external_browser_layer,
	preview_browser_opener,
	PreviewExternalBrowser,
	PreviewInspection,
	PreviewInspectionConnector,
	PreviewInspectionConnectorError,
	PreviewInspectionConnectorUnavailableLive,
	type PreviewInspectionConnectorHandle,
} from "../../modules/backend/src/preview/runtime";

const closeable_servers: Array<ReturnType<typeof createServer>> = [];

afterEach(async () => {
	await Promise.all(
		closeable_servers
			.splice(0)
			.map((server) => new Promise<void>((resolve) => server.close(() => resolve()))),
	);
});

function target(url: string): PreviewTargetRecord {
	return {
		created_at_ms: 1,
		health: Option.none(),
		id: "preview-runtime",
		project_id: "project-runtime",
		source: Option.none(),
		state: "registered",
		updated_at_ms: 1,
		url,
		workspace_id: "workspace-runtime",
	};
}

async function listen(handler: (request: IncomingMessage, response: ServerResponse) => void) {
	const server = createServer(handler);

	closeable_servers.push(server);
	await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", () => resolve()));
	const address = server.address();

	if (address === null || typeof address === "string") {
		throw new Error("expected a TCP preview listener");
	}

	return `http://127.0.0.1:${address.port}/health`;
}

describe("preview runtime", () => {
	it("probes a direct loopback target with bounded body, no credentials, and status evidence", async () => {
		let request_headers: Record<string, string | string[] | undefined> = {};
		const bound_url = await listen((request, response) => {
			request_headers = request.headers;
			response.writeHead(204, { "content-type": "text/plain" });
			response.end();
		});
		const url = bound_url.replace("127.0.0.1", "localhost");
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const probe = yield* PreviewHealthProbe;

					return yield* probe.Probe(target(url));
				}).pipe(Effect.provide(make_node_preview_health_probe_layer())),
			),
		);

		expect(result.status).toBe("healthy");
		expect(result.status_code).toEqual(Option.some(204));
		expect(request_headers.cookie).toBeUndefined();
		expect(request_headers.authorization).toBeUndefined();
	});

	it("rejects non-loopback health targets before any network request", async () => {
		const error = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const probe = yield* PreviewHealthProbe;

					return yield* probe.Probe(target("https://example.com/")).pipe(Effect.flip);
				}).pipe(Effect.provide(make_node_preview_health_probe_layer())),
			),
		);

		expect(error.target_id).toBe("preview-runtime");
	});

	it("opens, inspects, and closes the exact attributable external connector session", async () => {
		let now_ms = 100;
		const calls: Array<string> = [];
		const handle = {} as PreviewInspectionConnectorHandle;
		const target_layer = make_preview_target_layer().pipe(
			Layer.provide(
				Layer.mergeAll(
					Layer.succeed(PreviewTargetClock, { Now: Effect.sync(() => now_ms++) }),
					Layer.succeed(PreviewHealthProbe, {
						Probe: () =>
							Effect.succeed({
								latency_ms: 4,
								message: Option.none(),
								status: "healthy" as const,
								status_code: Option.some(200),
							}),
					}),
				),
			),
		);
		const connector_layer = Layer.succeed(PreviewInspectionConnector, {
			Close: (received_handle: PreviewInspectionConnectorHandle) =>
				Effect.sync(() => {
					expect(received_handle).toBe(handle);
					calls.push("close");
				}),
			Inspect: (received_handle: PreviewInspectionConnectorHandle) =>
				Effect.sync(() => {
					expect(received_handle).toBe(handle);
					calls.push("inspect");
					return {
						latency_ms: 7,
						message: Option.some("connector observation"),
						status: "healthy" as const,
						status_code: Option.some(201),
					};
				}),
			Open: (input) =>
				Effect.sync(() => {
					expect(input.actor_id).toBe("agent-1");
					expect(input.connector_id).toBe("browser-debug-1");
					expect(input.target.id).toBe("preview-inspected");
					calls.push("open");
					return handle;
				}),
		});
		const layer = Layer.merge(
			target_layer,
			make_preview_inspection_layer().pipe(
				Layer.provide(Layer.mergeAll(target_layer, connector_layer)),
			),
		);
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const targets = yield* PreviewTarget;
					const inspection = yield* PreviewInspection;

					yield* targets.Register({
						id: "preview-inspected",
						project_id: "project-inspected",
						url: "http://localhost:4173/",
						workspace_id: "workspace-inspected",
					});
					const session = yield* inspection.Open({
						actor_id: "agent-1",
						connector_id: "browser-debug-1",
						target_id: "preview-inspected",
					});
					const observation = yield* inspection.Inspect(session.session_id);
					const sessions = yield* inspection.List;

					return { observation, sessions };
				}).pipe(Effect.provide(layer)),
			),
		);

		expect(result.observation).toMatchObject({ latency_ms: 7, status_code: Option.some(201) });
		expect(result.sessions).toMatchObject([
			{
				actor_id: "agent-1",
				connector_id: "browser-debug-1",
				target_id: "preview-inspected",
			},
		]);
		expect(calls).toEqual(["open", "inspect", "close"]);
	});

	it("fails closed when no external inspection connector is available", async () => {
		let now_ms = 100;
		const target_layer = make_preview_target_layer().pipe(
			Layer.provide(
				Layer.mergeAll(
					Layer.succeed(PreviewTargetClock, { Now: Effect.sync(() => now_ms++) }),
					Layer.succeed(PreviewHealthProbe, {
						Probe: () => Effect.die("must not probe directly"),
					}),
				),
			),
		);
		const layer = Layer.merge(
			target_layer,
			make_preview_inspection_layer().pipe(
				Layer.provide(
					Layer.mergeAll(target_layer, PreviewInspectionConnectorUnavailableLive),
				),
			),
		);
		const failure = await Effect.runPromise(
			Effect.gen(function* () {
				const targets = yield* PreviewTarget;
				const inspection = yield* PreviewInspection;

				yield* targets.Register({
					id: "preview-unavailable",
					project_id: "project-unavailable",
					url: "http://localhost:4173/",
					workspace_id: "workspace-unavailable",
				});

				return yield* inspection
					.Open({
						actor_id: "agent-1",
						connector_id: "browser-debug-1",
						target_id: "preview-unavailable",
					})
					.pipe(Effect.flip);
			}).pipe(Effect.provide(layer)),
		);

		expect(failure.code).toBe("connector_unavailable");
	});

	it("maps connector inspection failures without falling back to a target probe", async () => {
		let now_ms = 100;
		const handle = {} as PreviewInspectionConnectorHandle;
		const target_layer = make_preview_target_layer().pipe(
			Layer.provide(
				Layer.mergeAll(
					Layer.succeed(PreviewTargetClock, { Now: Effect.sync(() => now_ms++) }),
					Layer.succeed(PreviewHealthProbe, {
						Probe: () => Effect.die("must not probe directly"),
					}),
				),
			),
		);
		const connector_layer = Layer.succeed(PreviewInspectionConnector, {
			Close: () => Effect.void,
			Inspect: () =>
				Effect.fail(
					new PreviewInspectionConnectorError({
						cause: new Error("connector detached"),
						code: "failed",
						target_id: "preview-failed",
					}),
				),
			Open: () => Effect.succeed(handle),
		});
		const layer = Layer.merge(
			target_layer,
			make_preview_inspection_layer().pipe(
				Layer.provide(Layer.mergeAll(target_layer, connector_layer)),
			),
		);
		const failure = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const targets = yield* PreviewTarget;
					const inspection = yield* PreviewInspection;

					yield* targets.Register({
						id: "preview-failed",
						project_id: "project-failed",
						url: "http://localhost:4173/",
						workspace_id: "workspace-failed",
					});
					const session = yield* inspection.Open({
						actor_id: "agent-1",
						connector_id: "browser-debug-1",
						target_id: "preview-failed",
					});

					return yield* inspection.Inspect(session.session_id).pipe(Effect.flip);
				}).pipe(Effect.provide(layer)),
			),
		);

		expect(failure.code).toBe("connector_failed");
	});

	it("closes a remaining connector handle once when its layer finalizes", async () => {
		let now_ms = 100;
		let close_count = 0;
		const handle = {} as PreviewInspectionConnectorHandle;
		const target_layer = make_preview_target_layer().pipe(
			Layer.provide(
				Layer.mergeAll(
					Layer.succeed(PreviewTargetClock, { Now: Effect.sync(() => now_ms++) }),
					Layer.succeed(PreviewHealthProbe, {
						Probe: () => Effect.die("must not probe directly"),
					}),
				),
			),
		);
		const connector_layer = Layer.succeed(PreviewInspectionConnector, {
			Close: () => Effect.sync(() => void (close_count += 1)),
			Inspect: () => Effect.die("not inspected"),
			Open: () => Effect.succeed(handle),
		});
		const layer = Layer.merge(
			target_layer,
			make_preview_inspection_layer().pipe(
				Layer.provide(Layer.mergeAll(target_layer, connector_layer)),
			),
		);

		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const targets = yield* PreviewTarget;
					const inspection = yield* PreviewInspection;

					yield* targets.Register({
						id: "preview-finalized",
						project_id: "project-finalized",
						url: "http://localhost:4173/",
						workspace_id: "workspace-finalized",
					});
					yield* inspection.Open({
						actor_id: "agent-1",
						connector_id: "browser-debug-1",
						target_id: "preview-finalized",
					});
				}).pipe(Effect.provide(layer)),
			),
		);

		expect(close_count).toBe(1);
	});

	it("uses only fixed, shell-free OS URL handlers and never models an embedded browser", () => {
		expect(preview_browser_opener("win32", "http://localhost:5173/")).toEqual({
			args: ["url.dll,FileProtocolHandler", "http://localhost:5173/"],
			command: "rundll32.exe",
		});
		expect(preview_browser_opener("darwin", "http://localhost:5173/")?.command).toBe("open");
		expect(preview_browser_opener("linux", "http://localhost:5173/")?.command).toBe("xdg-open");
		expect(preview_browser_opener("aix", "http://localhost:5173/")).toBeUndefined();
	});

	it("keeps existing targets retryably browser-unavailable when no OS opener exists", async () => {
		let now_ms = 100;
		const target_layer = make_preview_target_layer().pipe(
			Layer.provide(
				Layer.mergeAll(
					Layer.succeed(PreviewTargetClock, { Now: Effect.sync(() => now_ms++) }),
					Layer.succeed(PreviewHealthProbe, {
						Probe: () => Effect.die("not probed"),
					}),
				),
			),
		);
		const spawner_layer = Layer.succeed(ChildProcessSpawner.ChildProcessSpawner, {
			spawn: () => Effect.die("unsupported platforms must not spawn"),
		} as never);
		const browser_layer = make_preview_external_browser_layer("aix").pipe(
			Layer.provide(Layer.mergeAll(target_layer, spawner_layer)),
		);
		const layer = Layer.merge(target_layer, browser_layer);

		const failure = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const targets = yield* PreviewTarget;
					const browser = yield* PreviewExternalBrowser;

					yield* targets.Register({
						id: "preview-existing-no-opener",
						project_id: "project-runtime",
						url: "http://localhost:4173/",
						workspace_id: "workspace-runtime",
					});
					return yield* browser
						.Launch({ actor_id: "agent-1", target_id: "preview-existing-no-opener" })
						.pipe(Effect.flip);
				}).pipe(Effect.provide(layer)),
			),
		);

		expect(failure).toMatchObject({
			code: "browser_unavailable",
			target_id: "preview-existing-no-opener",
		});
	});
});
