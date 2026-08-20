import { mkdtemp, rm } from "node:fs/promises";
import { createHash } from "node:crypto";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { Deferred, Effect, Fiber, Layer, Option, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";
import {
	make_backend_runtime,
	PreviewExternalBrowser,
	PreviewHealthProbe,
	PreviewHealthProbeError,
	PreviewInspectionConnector,
	PreviewRuntimeError,
	type PreviewInspectionConnectorHandle,
	PreviewCoordinator,
	RichLinkAssetStore,
	RichLinkMetadata,
	ProtocolServer,
	ThreadErasure,
} from "@artisan/backend";
import type { OutboundControlEnvelope } from "@artisan/protocol";
import { make_transport_test_harness_with_protocol_server } from "./message-channel-harness";
import { Database } from "../../modules/backend/src/persistence/database";
import { ThreadErasureClaims } from "../../modules/backend/src/persistence/tables";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: string[] = [];
const preview_inspection_connector = Layer.succeed(PreviewInspectionConnector, {
	Close: () => Effect.void,
	Inspect: () =>
		Effect.succeed({
			latency_ms: 1,
			message: Option.some("fixture connector health"),
			status: "healthy" as const,
			status_code: Option.some(200),
		}),
	Open: () => Effect.succeed({} as PreviewInspectionConnectorHandle),
});
const TakeExactReply = (
	connection: { readonly Outbound: Stream.Stream<OutboundControlEnvelope> },
	message_id: string,
	kind: "preview.browser.launch.result" | "protocol.error",
) =>
	connection.Outbound.pipe(
		Stream.takeUntil(
			(envelope) =>
				envelope.kind === kind &&
				"correlation_id" in envelope &&
				envelope.correlation_id === message_id,
		),
		Stream.runCollect,
		Effect.map((outbound) => {
			const observed = Array.from(outbound);

			return {
				interleaved_events: observed.filter((envelope) => envelope.kind === "event"),
				reply: observed.at(-1),
			};
		}),
	);
const RawLaunchTwice = (
	server: typeof ProtocolServer.Service,
	message_id: string,
	target_id: string,
) =>
	Effect.scoped(
		Effect.gen(function* () {
			const connection = yield* server.Open;
			yield* connection.Receive({
				kind: "hello",
				message_id: `${message_id}-hello`,
				origin: "frontend",
				payload: {
					event_cursors: [],
					last_journal_sequence: 0,
					supported_protocol_versions: [1],
				},
				schema_version: 1,
				sent_at: "2026-07-18T20:00:00.000Z",
			});
			yield* connection.Outbound.pipe(
				Stream.takeUntil((envelope) => envelope.kind === "replay.complete"),
				Stream.runDrain,
			);
			const launch = {
				kind: "preview.browser.launch" as const,
				message_id,
				origin: "frontend" as const,
				payload: { target_id },
				protocol_version: 1 as const,
				schema_version: 1 as const,
				sent_at: "2026-07-18T20:00:01.000Z",
			};
			yield* connection.Receive(launch);
			const first = yield* TakeExactReply(connection, message_id, "protocol.error");
			yield* connection.Receive(launch);
			const replay = yield* TakeExactReply(connection, message_id, "protocol.error");
			return { first, replay };
		}),
	);
afterEach(async () =>
	Promise.all(
		directories.splice(0).map((directory) => rm(directory, { force: true, recursive: true })),
	),
);

describe("preview public MessagePort protocol", () => {
	it("returns rich-link metadata without leaking internal timestamp fields", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-preview-rich-link-"));
		directories.push(directory);
		const fetched_at_ms = Date.parse("2026-07-18T20:00:00.000Z");
		const favicon_body = Uint8Array.of(137, 80, 78, 71, 13, 10, 26, 10);
		const favicon_asset_id = "a".repeat(64);
		const runtime = make_backend_runtime({
			database_path: join(directory, "artisan.db"),
			migrations_path,
			preview_rich_links: Layer.mergeAll(
				Layer.succeed(RichLinkAssetStore, {
					Get: (asset_id) =>
						Effect.succeed(
							asset_id === favicon_asset_id
								? Option.some({
										asset_id,
										body: favicon_body,
										bytes: favicon_body.byteLength,
										content_type: "image/png",
									})
								: Option.none(),
						),
					limits: { max_entries: 1, max_total_bytes: 1024 },
					Put: () => Effect.die("unused fixture asset store"),
				}),
				Layer.succeed(RichLinkMetadata, {
					ResolveImage: () => Effect.die("not used"),
					Resolve: () =>
						Effect.succeed({
							cache: {
								expires_at_ms: fetched_at_ms + 60_000,
								status: "miss" as const,
							},
							favicon: Option.some({
								asset_id: favicon_asset_id,
								bytes: favicon_body.byteLength,
								content_type: "image/png",
								source: "document_icon" as const,
								source_url: "https://example.com/favicon.png",
							}),
							fetched_at_ms,
							final_url: "https://example.com/docs",
							page_name: "Example documentation",
							requested_url: "https://example.com/docs",
							site_name: "Example",
							title: Option.some("Document title"),
						}),
				}),
			),
		});
		const server = await runtime.runPromise(ProtocolServer);
		const harness = await make_transport_test_harness_with_protocol_server(server, {
			binary_streams: { [`asset:${favicon_asset_id}`]: [favicon_body] },
		});
		try {
			const result = await Effect.runPromise(
				harness.client.ResolveRichLink({ url: "https://example.com/docs" }),
			);

			expect(result).toEqual({
				cache: { expires_at: "2026-07-18T20:01:00.000Z", status: "miss" },
				favicon: {
					asset_id: favicon_asset_id,
					bytes: favicon_body.byteLength,
					content_type: "image/png",
					source: "document_icon",
					source_url: "https://example.com/favicon.png",
				},
				fetched_at: "2026-07-18T20:00:00.000Z",
				final_url: "https://example.com/docs",
				page_name: "Example documentation",
				requested_url: "https://example.com/docs",
				site_name: "Example",
				title: "Document title",
			});
			expect(result).not.toHaveProperty("fetched_at_ms");
			const chunks = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const stream = yield* harness.client.OpenAsset(
							result.favicon?.asset_id ?? "",
						);
						return yield* stream.pipe(Stream.runCollect);
					}),
				),
			);
			expect([...chunks].flatMap((chunk) => [...chunk])).toEqual([...favicon_body]);
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	});

	it("replays a failed launch envelope without reopening the browser", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-preview-launch-failure-"));
		directories.push(directory);
		let launches = 0;
		const runtime = make_backend_runtime({
			database_path: join(directory, "artisan.db"),
			migrations_path,
			preview_external_browser: Layer.succeed(PreviewExternalBrowser, {
				Launch: ({ target_id }) =>
					Effect.sync(() => void (launches += 1)).pipe(
						Effect.andThen(
							Effect.fail(
								new PreviewRuntimeError({
									cause: new Error("fixture browser unavailable"),
									code: "browser_unavailable",
									target_id,
								}),
							),
						),
					),
			}),
		});
		const server = await runtime.runPromise(ProtocolServer);
		const harness = await make_transport_test_harness_with_protocol_server(server);
		try {
			const created_preview_failed_launch_thread = await Effect.runPromise(
				harness.client.CreateThread({ title: "Failed launch" }),
			);
			await Effect.runPromise(
				harness.client.RegisterPreviewTarget({
					id: "preview-failed-launch-target",
					port: 4173,
					project_id: "preview-failed-launch-project",
					routes: ["/"],
					thread_id: created_preview_failed_launch_thread.thread_id,
					url: "http://127.0.0.1:4173/",
					workspace_id: "preview-failed-launch-workspace",
				}),
			);
			const result = await runtime.runPromise(
				RawLaunchTwice(
					server,
					"preview-exact-failed-launch",
					"preview-failed-launch-target",
				),
			);

			expect(launches).toBe(1);
			expect(result.first.reply).toMatchObject({
				kind: "protocol.error",
				payload: {
					code: "preview.browser_unavailable",
					message: "The external browser opener is currently unavailable.",
					retryable: true,
				},
			});
			expect(result.replay.reply).toMatchObject({
				kind: "protocol.error",
				payload: result.first.reply?.payload,
			});
			expect(result.replay.interleaved_events).toContainEqual(
				expect.objectContaining({
					kind: "event",
					payload: expect.objectContaining({ type: "preview.target.updated" }),
				}),
			);
			expect(
				await Effect.runPromise(
					harness.client.GetPreviewTarget({ target_id: "preview-failed-launch-target" }),
				),
			).toMatchObject({
				last_error: "external_browser_unavailable",
				launch_state: "error",
			});
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	});

	it("holds a cross-runtime lease through a blocked browser launch, then permits erasure after completion", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-preview-launch-lease-"));
		directories.push(directory);
		const entered = await Effect.runPromise(Deferred.make<void>());
		const release = await Effect.runPromise(Deferred.make<void>());
		let launches = 0;
		const options = {
			database_path: join(directory, "artisan.db"),
			migrations_path,
			preview_external_browser: Layer.succeed(PreviewExternalBrowser, {
				Launch: () =>
					Effect.gen(function* () {
						launches += 1;
						yield* Deferred.succeed(entered, undefined);
						yield* Deferred.await(release);
					}),
			}),
			preview_inspection_connector,
		};
		const runtime_a = make_backend_runtime(options);
		const runtime_b = make_backend_runtime(options);
		const server_a = await runtime_a.runPromise(ProtocolServer);
		const harness_a = await make_transport_test_harness_with_protocol_server(server_a);
		try {
			const created_preview_launch_lease_thread = await Effect.runPromise(
				harness_a.client.CreateThread({ title: "Preview lease" }),
			);
			await Effect.runPromise(
				harness_a.client.RegisterPreviewTarget({
					id: "preview-launch-lease-target",
					port: 5173,
					project_id: "project",
					routes: [],
					thread_id: created_preview_launch_lease_thread.thread_id,
					url: "http://localhost:5173/",
					workspace_id: "workspace",
				}),
			);
			const coordinator_a = await runtime_a.runPromise(PreviewCoordinator);
			const launch = runtime_a.runFork(
				coordinator_a.Launch({
					message_id: "preview-launch-lease",
					target_id: "preview-launch-lease-target",
				}),
			);
			await Effect.runPromise(Deferred.await(entered));
			const deferred_erasure = await runtime_b.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const erasure = yield* ThreadErasure;
					yield* database.client.insert(ThreadErasureClaims).values({
						claimed_at: new Date().toISOString(),
						thread_id: created_preview_launch_lease_thread.thread_id,
					});
					return yield* erasure.ResumeClaimed(new Date().toISOString());
				}),
			);
			expect(deferred_erasure).toEqual([]);
			expect(launches).toBe(1);
			await Effect.runPromise(Deferred.succeed(release, undefined));
			await runtime_a.runPromise(Fiber.join(launch));
			const erased = await runtime_b.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const erasure = yield* ThreadErasure;
					yield* database.client.insert(ThreadErasureClaims).values({
						claimed_at: new Date().toISOString(),
						thread_id: created_preview_launch_lease_thread.thread_id,
					});
					return yield* erasure.ResumeClaimed(new Date().toISOString());
				}),
			);
			expect(erased).toEqual([created_preview_launch_lease_thread.thread_id]);
			expect(launches).toBe(1);
		} finally {
			await harness_a.dispose();
			await runtime_a.dispose();
			await runtime_b.dispose();
		}
	});

	it.each(["probe", "open", "inspect"] as const)(
		"fences cross-runtime erasure while a blocked preview %s side effect is leased",
		async (operation) => {
			const directory = await mkdtemp(join(tmpdir(), `artisan-preview-${operation}-lease-`));
			directories.push(directory);
			const entered = await Effect.runPromise(Deferred.make<void>());
			const release = await Effect.runPromise(Deferred.make<void>());
			let effects = 0;
			let block = false;
			const Block = <A>(value: A) =>
				Effect.gen(function* () {
					effects += 1;
					if (block) {
						yield* Deferred.succeed(entered, undefined);
						yield* Deferred.await(release);
					}
					return value;
				});
			const options = {
				database_path: join(directory, "artisan.db"),
				migrations_path,
				preview_health_probe: Layer.succeed(PreviewHealthProbe, {
					Probe: () =>
						Block({
							latency_ms: 1,
							message: Option.none(),
							status: "healthy" as const,
							status_code: Option.some(200),
						}),
				}),
				preview_inspection_connector: Layer.succeed(PreviewInspectionConnector, {
					Close: () => Effect.void,
					Inspect: () =>
						Block({
							latency_ms: 1,
							message: Option.none(),
							status: "healthy" as const,
							status_code: Option.some(200),
						}),
					Open: () => Block({} as PreviewInspectionConnectorHandle),
				}),
			};
			const runtime_a = make_backend_runtime(options);
			const runtime_b = make_backend_runtime(options);
			const server_a = await runtime_a.runPromise(ProtocolServer);
			const harness_a = await make_transport_test_harness_with_protocol_server(server_a);
			const target_id = `preview-${operation}-lease-target`;
			try {
				const created_thread = await Effect.runPromise(
					harness_a.client.CreateThread({ title: "Preview lease" }),
				);
				const thread_id = created_thread.thread_id;
				await Effect.runPromise(
					harness_a.client.RegisterPreviewTarget({
						id: target_id,
						port: 5173,
						project_id: "project",
						routes: [],
						thread_id,
						url: "http://localhost:5173/",
						workspace_id: "workspace",
					}),
				);
				const coordinator = await runtime_a.runPromise(PreviewCoordinator);
				const session =
					operation === "inspect"
						? await runtime_a.runPromise(
								coordinator.OpenInspection({
									connector_id: "connector",
									message_id: `${thread_id}-initial-open`,
									target_id,
								}),
							)
						: undefined;
				block = true;
				const active = runtime_a.runFork(
					operation === "probe"
						? coordinator
								.Probe({ message_id: `${thread_id}-effect`, target_id })
								.pipe(Effect.asVoid)
						: operation === "open"
							? coordinator
									.OpenInspection({
										connector_id: "connector",
										message_id: `${thread_id}-effect`,
										target_id,
									})
									.pipe(Effect.asVoid)
							: coordinator
									.Inspect({
										message_id: `${thread_id}-effect`,
										operation: "health",
										session_id: session!.session_id,
									})
									.pipe(Effect.asVoid),
				);
				await Effect.runPromise(Deferred.await(entered));
				const deferred = await runtime_b.runPromise(
					Effect.gen(function* () {
						const database = yield* Database;
						const erasure = yield* ThreadErasure;
						yield* database.client.insert(ThreadErasureClaims).values({
							claimed_at: new Date().toISOString(),
							thread_id,
						});
						return yield* erasure.ResumeClaimed(new Date().toISOString());
					}),
				);
				expect(deferred).toEqual([]);
				await Effect.runPromise(Deferred.succeed(release, undefined));
				await runtime_a.runPromise(Fiber.join(active));
				const erased = await runtime_b.runPromise(
					Effect.gen(function* () {
						const database = yield* Database;
						const erasure = yield* ThreadErasure;
						yield* database.client.insert(ThreadErasureClaims).values({
							claimed_at: new Date().toISOString(),
							thread_id,
						});
						return yield* erasure.ResumeClaimed(new Date().toISOString());
					}),
				);
				expect(erased).toEqual([thread_id]);
				expect(effects).toBe(operation === "inspect" ? 2 : 1);
			} finally {
				await harness_a.dispose();
				await runtime_a.dispose();
				await runtime_b.dispose();
			}
		},
	);
	it("reports an existing target's browser opener failure as retryable availability", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-preview-browser-unavailable-"));
		directories.push(directory);
		const runtime = make_backend_runtime({
			database_path: join(directory, "artisan.db"),
			migrations_path,
			preview_external_browser: Layer.succeed(PreviewExternalBrowser, {
				Launch: ({ target_id }) =>
					Effect.fail(
						new PreviewRuntimeError({
							cause: new Error("configured browser opener is unavailable"),
							code: "browser_unavailable",
							target_id,
						}),
					),
			}),
			preview_inspection_connector,
		});
		const server = await runtime.runPromise(ProtocolServer);
		const harness = await make_transport_test_harness_with_protocol_server(server);
		try {
			const created_preview_browser_unavailable_thread = await Effect.runPromise(
				harness.client.CreateThread({ title: "Preview" }),
			);
			await Effect.runPromise(
				harness.client.RegisterPreviewTarget({
					id: "preview-browser-unavailable-target",
					port: 4173,
					project_id: "preview-browser-unavailable-project",
					routes: ["/"],
					thread_id: created_preview_browser_unavailable_thread.thread_id,
					url: "http://127.0.0.1:4173/",
					workspace_id: "preview-browser-unavailable-workspace",
				}),
			);

			await expect(
				Effect.runPromise(
					harness.client.LaunchPreviewInExternalBrowser({
						target_id: "preview-browser-unavailable-target",
					}),
				),
			).rejects.toMatchObject({
				protocol_code: "preview.browser_unavailable",
				retryable: true,
			});
			expect(
				await Effect.runPromise(
					harness.client.GetPreviewTarget({
						target_id: "preview-browser-unavailable-target",
					}),
				),
			).toMatchObject({
				launch_state: "error",
				last_error: "external_browser_unavailable",
			});
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	});

	it("fences register, probe, and inspection calls after preview quiescence begins", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-preview-quiesce-"));
		directories.push(directory);
		let connector_closes = 0;
		let connector_inspects = 0;
		let health_probes = 0;
		const runtime = make_backend_runtime({
			database_path: join(directory, "artisan.db"),
			migrations_path,
			preview_health_probe: Layer.succeed(PreviewHealthProbe, {
				Probe: () =>
					Effect.sync(() => {
						health_probes += 1;
						return {
							latency_ms: 1,
							message: Option.none(),
							status: "healthy" as const,
							status_code: Option.some(200),
						};
					}),
			}),
			preview_inspection_connector: Layer.succeed(PreviewInspectionConnector, {
				Close: () => Effect.sync(() => void (connector_closes += 1)),
				Inspect: () =>
					Effect.sync(() => {
						connector_inspects += 1;
						return {
							latency_ms: 1,
							message: Option.none(),
							status: "healthy" as const,
							status_code: Option.some(200),
						};
					}),
				Open: () => Effect.succeed({} as PreviewInspectionConnectorHandle),
			}),
		});
		const server = await runtime.runPromise(ProtocolServer);
		const harness = await make_transport_test_harness_with_protocol_server(server);
		try {
			const created_preview_quiesce_thread = await Effect.runPromise(
				harness.client.CreateThread({ title: "Preview quiesce" }),
			);
			await Effect.runPromise(
				harness.client.RegisterPreviewTarget({
					id: "preview-quiesce-target",
					port: 4173,
					project_id: "preview-quiesce-project",
					routes: ["/"],
					thread_id: created_preview_quiesce_thread.thread_id,
					url: "http://127.0.0.1:4173/",
					workspace_id: "preview-quiesce-workspace",
				}),
			);
			const inspection = await Effect.runPromise(
				harness.client.OpenPreviewInspectionSession({
					connector_id: "preview-quiesce-connector",
					target_id: "preview-quiesce-target",
				}),
			);
			await runtime.runPromise(
				Effect.flatMap(PreviewCoordinator, (coordinator) =>
					coordinator.QuiesceThread(created_preview_quiesce_thread.thread_id),
				),
			);

			await expect(
				Effect.runPromise(
					harness.client.ProbePreviewTarget({ target_id: "preview-quiesce-target" }),
				),
			).rejects.toMatchObject({ protocol_code: "preview.not_found" });
			await expect(
				Effect.runPromise(
					harness.client.InspectPreviewSession({
						operation: "health",
						session_id: inspection.session_id,
					}),
				),
			).rejects.toMatchObject({ protocol_code: "preview.not_found" });
			await expect(
				Effect.runPromise(
					harness.client.RegisterPreviewTarget({
						id: "preview-quiesce-late-target",
						port: 4174,
						project_id: "preview-quiesce-project",
						routes: ["/"],
						thread_id: created_preview_quiesce_thread.thread_id,
						url: "http://127.0.0.1:4174/",
						workspace_id: "preview-quiesce-workspace",
					}),
				),
			).rejects.toMatchObject({ protocol_code: "preview.not_found" });
			expect({ connector_closes, connector_inspects, health_probes }).toEqual({
				connector_closes: 1,
				connector_inspects: 0,
				health_probes: 0,
			});
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	});

	it("closes inspection connectors and removes runtime targets before thread erasure", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-preview-erasure-"));
		directories.push(directory);
		let connector_closes = 0;
		const runtime = make_backend_runtime({
			database_path: join(directory, "artisan.db"),
			migrations_path,
			preview_inspection_connector: Layer.succeed(PreviewInspectionConnector, {
				Close: () =>
					Effect.sync(() => {
						connector_closes += 1;
					}),
				Inspect: () =>
					Effect.succeed({
						latency_ms: 1,
						message: Option.none(),
						status: "healthy" as const,
						status_code: Option.some(200),
					}),
				Open: () => Effect.succeed({} as PreviewInspectionConnectorHandle),
			}),
		});
		const server = await runtime.runPromise(ProtocolServer);
		const harness = await make_transport_test_harness_with_protocol_server(server);
		try {
			const created_preview_erasure_thread = await Effect.runPromise(
				harness.client.CreateThread({ title: "Preview erasure" }),
			);
			await Effect.runPromise(
				harness.client.RegisterPreviewTarget({
					id: "preview-erasure-target",
					port: 4173,
					project_id: "preview-erasure-project",
					routes: ["/"],
					thread_id: created_preview_erasure_thread.thread_id,
					url: "http://127.0.0.1:4173/",
					workspace_id: "preview-erasure-workspace",
				}),
			);
			await Effect.runPromise(
				harness.client.OpenPreviewInspectionSession({
					connector_id: "preview-erasure-connector",
					target_id: "preview-erasure-target",
				}),
			);

			const erased = await runtime.runPromise(
				Effect.gen(function* () {
					const erasure = yield* ThreadErasure;
					return yield* erasure.CleanupExpired(
						"2027-01-01T00:00:00.000Z",
						"2027-01-01T00:00:00.000Z",
					);
				}),
			);

			expect(erased).toContain(created_preview_erasure_thread.thread_id);
			expect(connector_closes).toBe(1);
			await expect(
				Effect.runPromise(
					harness.client.GetPreviewTarget({ target_id: "preview-erasure-target" }),
				),
			).rejects.toMatchObject({ protocol_code: "preview.not_found" });
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	});

	it("fences launch and inspection side effects once erasure claims the thread", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-preview-erasure-race-"));
		directories.push(directory);
		let browser_launches = 0;
		let connector_closes = 0;
		let connector_opens = 0;
		const runtime = make_backend_runtime({
			database_path: join(directory, "artisan.db"),
			migrations_path,
			preview_external_browser: Layer.succeed(PreviewExternalBrowser, {
				Launch: () =>
					Effect.sync(() => {
						browser_launches += 1;
					}),
			}),
			preview_inspection_connector: Layer.succeed(PreviewInspectionConnector, {
				Close: () =>
					Effect.sync(() => {
						connector_closes += 1;
					}),
				Inspect: () =>
					Effect.succeed({
						latency_ms: 1,
						message: Option.none(),
						status: "healthy" as const,
						status_code: Option.some(200),
					}),
				Open: () =>
					Effect.sync(() => {
						connector_opens += 1;
						return {} as PreviewInspectionConnectorHandle;
					}),
			}),
		});
		const server = await runtime.runPromise(ProtocolServer);
		const harness = await make_transport_test_harness_with_protocol_server(server);
		try {
			const created_preview_erasure_race_thread = await Effect.runPromise(
				harness.client.CreateThread({ title: "Preview erasure race" }),
			);
			await Effect.runPromise(
				harness.client.RegisterPreviewTarget({
					id: "preview-erasure-race-target",
					port: 4173,
					project_id: "preview-erasure-race-project",
					routes: ["/"],
					thread_id: created_preview_erasure_race_thread.thread_id,
					url: "http://127.0.0.1:4173/",
					workspace_id: "preview-erasure-race-workspace",
				}),
			);
			await Effect.runPromise(
				harness.client.OpenPreviewInspectionSession({
					connector_id: "preview-erasure-race-existing",
					target_id: "preview-erasure-race-target",
				}),
			);
			await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					yield* database.client.insert(ThreadErasureClaims).values({
						claimed_at: "2027-01-01T00:00:00.000Z",
						thread_id: created_preview_erasure_race_thread.thread_id,
					});
				}),
			);

			await expect(
				Effect.runPromise(
					harness.client.LaunchPreviewInExternalBrowser({
						target_id: "preview-erasure-race-target",
					}),
				),
			).rejects.toMatchObject({ protocol_code: "preview.not_found" });
			await expect(
				Effect.runPromise(
					harness.client.OpenPreviewInspectionSession({
						connector_id: "preview-erasure-race-late",
						target_id: "preview-erasure-race-target",
					}),
				),
			).rejects.toMatchObject({ protocol_code: "preview.not_found" });
			expect(browser_launches).toBe(0);
			expect(connector_opens).toBe(1);

			await runtime.runPromise(
				Effect.gen(function* () {
					const erasure = yield* ThreadErasure;
					yield* erasure.ResumeClaimed("2027-01-01T00:00:01.000Z");
				}),
			);
			expect(connector_closes).toBe(1);
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	});

	it("hydrates durable targets and abandons open inspection sessions after restart", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-preview-restart-"));
		directories.push(directory);
		const options = {
			database_path: join(directory, "artisan.db"),
			migrations_path,
			preview_inspection_connector,
		};
		const runtime1 = make_backend_runtime(options);
		const server1 = await runtime1.runPromise(ProtocolServer);
		const harness1 = await make_transport_test_harness_with_protocol_server(server1);
		try {
			const created_thread = await Effect.runPromise(
				harness1.client.CreateThread({ title: "Preview restart" }),
			);
			await Effect.runPromise(
				harness1.client.RegisterPreviewTarget({
					id: "preview-restart-target",
					port: 4173,
					project_id: "preview-restart-project",
					routes: ["/", "/health"],
					source: { kind: "process", process_id: "preview-restart-process" },
					thread_id: created_thread.thread_id,
					url: "http://127.0.0.1:4173",
					workspace_id: "preview-restart-workspace",
				}),
			);
			await Effect.runPromise(
				harness1.client.OpenPreviewInspectionSession({
					connector_id: "preview-restart-connector",
					target_id: "preview-restart-target",
				}),
			);
		} finally {
			await harness1.dispose();
			await runtime1.dispose();
		}
		const runtime2 = make_backend_runtime(options);
		const server2 = await runtime2.runPromise(ProtocolServer);
		const harness2 = await make_transport_test_harness_with_protocol_server(server2);
		try {
			expect(
				await Effect.runPromise(
					harness2.client.GetPreviewTarget({ target_id: "preview-restart-target" }),
				),
			).toMatchObject({
				routes: ["/", "/health"],
				source: { kind: "process", process_id: "preview-restart-process" },
			});
		} finally {
			await harness2.dispose();
			await runtime2.dispose();
		}
	});

	it("delivers contiguous preview target and inspection events to a scoped live client collector", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-preview-events-"));
		directories.push(directory);
		const runtime = make_backend_runtime({
			database_path: join(directory, "artisan.db"),
			migrations_path,
			preview_inspection_connector,
		});
		const server = await runtime.runPromise(ProtocolServer);
		const harness = await make_transport_test_harness_with_protocol_server(server);
		try {
			const output = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const collector = yield* harness.client.Events.pipe(
							Stream.filter(
								(event) =>
									event.payload.type === "preview.target.updated" ||
									event.payload.type === "preview.inspection.updated",
							),
							Stream.take(2),
							Stream.runCollect,
							Effect.forkChild,
						);
						yield* Effect.yieldNow;
						const created_thread = yield* harness.client.CreateThread({
							title: "Preview events",
						});
						yield* harness.client.RegisterPreviewTarget({
							id: "preview-events-target",
							port: 4173,
							project_id: "preview-events-project",
							routes: ["/"],
							thread_id: created_thread.thread_id,
							url: "http://127.0.0.1:4173",
							workspace_id: "preview-events-workspace",
						});
						yield* harness.client.OpenPreviewInspectionSession({
							connector_id: "preview-events-connector",
							target_id: "preview-events-target",
						});
						return {
							events: yield* Fiber.join(collector),
							thread_id: created_thread.thread_id,
						};
					}),
				),
			);
			const [target, inspection] = [...output.events] as [
				NonNullable<(typeof output.events)[number]>,
				NonNullable<(typeof output.events)[number]>,
			];
			expect(target).toMatchObject({
				payload: { type: "preview.target.updated" },
				sequence: 2,
				stream_id: `thread:${output.thread_id}`,
			});
			expect(inspection).toMatchObject({
				payload: { type: "preview.inspection.updated" },
				sequence: 3,
				stream_id: `thread:${output.thread_id}`,
			});
			expect(inspection.journal_sequence).toBe(target.journal_sequence + 1);
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	});

	it("persists a target with source, port, and routes", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-preview-port-"));
		directories.push(directory);
		let health_status: "healthy" | "unhealthy" = "healthy";
		let health_failure = false;
		let browser_launches = 0;
		const asset_body = new TextEncoder().encode("fixture asset");
		const asset_id = createHash("sha256").update(asset_body).digest("hex");
		const runtime = make_backend_runtime({
			database_path: join(directory, "artisan.db"),
			migrations_path,
			preview_inspection_connector,
			preview_external_browser: Layer.succeed(PreviewExternalBrowser, {
				Launch: () =>
					Effect.sync(() => {
						browser_launches += 1;
					}),
			}),
			preview_health_probe: Layer.succeed(PreviewHealthProbe, {
				Probe: () =>
					health_failure
						? Effect.fail(
								new PreviewHealthProbeError({
									cause: new Error("fixture health failure"),
									target_id: "preview-target",
								}),
							)
						: Effect.succeed({
								latency_ms: 3,
								message: Option.some("fixture health"),
								status: health_status,
								status_code: Option.some(health_status === "healthy" ? 200 : 503),
							}),
			}),
			preview_rich_links: Layer.mergeAll(
				Layer.succeed(RichLinkAssetStore, {
					Get: (requested_asset_id: string) =>
						Effect.succeed(
							requested_asset_id === asset_id
								? Option.some({
										asset_id,
										body: asset_body,
										bytes: asset_body.byteLength,
										content_type: "image/png",
									})
								: Option.none(),
						),
					limits: { max_entries: 1, max_total_bytes: 1024 },
					Put: () => Effect.die("unused fixture asset store"),
				}),
				Layer.succeed(RichLinkMetadata, {
					ResolveImage: () => Effect.die("unused fixture image"),
					Resolve: () => Effect.die("unused fixture metadata"),
				}),
			),
		});
		const server = await runtime.runPromise(ProtocolServer);
		const harness = await make_transport_test_harness_with_protocol_server(server, {
			binary_streams: { [`asset:${asset_id}`]: [asset_body] },
		});
		try {
			const created_preview_thread = await Effect.runPromise(
				harness.client.CreateThread({ title: "Preview" }),
			);
			const target = await Effect.runPromise(
				harness.client.RegisterPreviewTarget({
					id: "preview-target",
					port: 4173,
					project_id: "preview-project",
					routes: ["/", "/health"],
					source: { kind: "process", process_id: "preview-process" },
					thread_id: created_preview_thread.thread_id,
					url: "http://127.0.0.1:4173",
					workspace_id: "preview-workspace",
				}),
			);
			expect(target).toMatchObject({
				port: 4173,
				routes: ["/", "/health"],
				source: { kind: "process", process_id: "preview-process" },
			});
			expect(
				await Effect.runPromise(harness.client.GetPreviewAssetMetadata({ asset_id })),
			).toEqual({ asset_id, bytes: asset_body.byteLength, content_type: "image/png" });
			const asset_chunks = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const stream = yield* harness.client.OpenAsset(asset_id);
						return yield* stream.pipe(Stream.runCollect);
					}),
				),
			);
			expect([...asset_chunks].flatMap((chunk) => [...chunk])).toEqual([...asset_body]);
			await expect(
				Effect.runPromise(
					harness.client.GetPreviewAssetMetadata({ asset_id: "0".repeat(64) }),
				),
			).rejects.toMatchObject({ protocol_code: "preview.not_found", retryable: false });
			await expect(
				Effect.runPromise(
					Effect.scoped(
						Effect.gen(function* () {
							const stream = yield* harness.client.OpenAsset("0".repeat(64));
							return yield* stream.pipe(Stream.runCollect);
						}),
					),
				),
			).rejects.toMatchObject({ code: "stream_not_found" });
			await expect(
				Effect.runPromise(
					Effect.scoped(
						Effect.gen(function* () {
							const stream = yield* harness.client.OpenAsset("0".repeat(64));
							return yield* stream.pipe(Stream.runCollect);
						}),
					),
				),
			).rejects.toMatchObject({ code: "stream_not_found", retryable: false });
			const post_failure_chunks = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const stream = yield* harness.client.OpenAsset(asset_id);
						return yield* stream.pipe(Stream.runCollect);
					}),
				),
			);
			expect([...post_failure_chunks].flatMap((chunk) => [...chunk])).toEqual([
				...asset_body,
			]);
			expect(harness.connector_snapshot().connections).toBe(1);
			expect(
				await Effect.runPromise(
					harness.client.GetPreviewTarget({ target_id: "preview-target" }),
				),
			).toMatchObject({ id: "preview-target", state: "registered" });
			expect(
				await Effect.runPromise(
					harness.client.ProbePreviewTarget({ target_id: "preview-target" }),
				),
			).toMatchObject({
				health: { message: "fixture health", status: "healthy", status_code: 200 },
				id: "preview-target",
				state: "healthy",
			});
			const inspection = await Effect.runPromise(
				harness.client.OpenPreviewInspectionSession({
					connector_id: "fixture-connector",
					target_id: "preview-target",
				}),
			);
			expect(inspection).toMatchObject({ state: "open", target_id: "preview-target" });
			expect(
				await Effect.runPromise(
					harness.client.InspectPreviewSession({
						operation: "metadata",
						session_id: inspection.session_id,
					}),
				),
			).toMatchObject({ operation: "metadata", session_id: inspection.session_id });
			expect(
				await Effect.runPromise(
					harness.client.InspectPreviewSession({
						operation: "health",
						session_id: inspection.session_id,
					}),
				),
			).toMatchObject({
				health: { status: "healthy", status_code: 200 },
				operation: "health",
			});
			expect(
				await Effect.runPromise(
					harness.client.ClosePreviewInspectionSession(inspection.session_id),
				),
			).toMatchObject({ session_id: inspection.session_id, state: "closed" });
			const exact_launch = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* server.Open;
						yield* connection.Receive({
							kind: "hello",
							message_id: "preview-launch-hello",
							origin: "frontend",
							payload: {
								event_cursors: [],
								last_journal_sequence: 0,
								supported_protocol_versions: [1],
							},
							schema_version: 1,
							sent_at: "2026-07-18T20:00:00.000Z",
						});
						yield* connection.Outbound.pipe(
							Stream.takeUntil((envelope) => envelope.kind === "replay.complete"),
							Stream.runDrain,
						);
						const launch = {
							kind: "preview.browser.launch" as const,
							message_id: "preview-exact-launch",
							origin: "frontend" as const,
							payload: { target_id: "preview-target" },
							protocol_version: 1 as const,
							schema_version: 1 as const,
							sent_at: "2026-07-18T20:00:01.000Z",
						};
						yield* connection.Receive(launch);
						const first = yield* TakeExactReply(
							connection,
							launch.message_id,
							"preview.browser.launch.result",
						);
						yield* connection.Receive(launch);
						const replay = yield* TakeExactReply(
							connection,
							launch.message_id,
							"preview.browser.launch.result",
						);
						return { first, replay };
					}),
				),
			);
			expect(exact_launch.first.reply).toMatchObject({
				kind: "preview.browser.launch.result",
				payload: { target_id: "preview-target" },
			});
			expect(exact_launch.replay.reply).toMatchObject({
				kind: "preview.browser.launch.result",
				payload: exact_launch.first.reply?.payload,
			});
			expect(exact_launch.replay.interleaved_events).toContainEqual(
				expect.objectContaining({
					kind: "event",
					payload: expect.objectContaining({ type: "preview.target.updated" }),
				}),
			);
			expect(browser_launches).toBe(1);
			expect(
				await Effect.runPromise(
					harness.client.GetPreviewTarget({ target_id: "preview-target" }),
				),
			).toMatchObject({ launch_state: "launched" });
			await expect(
				Effect.runPromise(
					harness.client.LaunchPreviewInExternalBrowser({ target_id: "preview-target" }),
				),
			).rejects.toMatchObject({ protocol_code: "preview.invalid" });
			expect(browser_launches).toBe(1);
			health_status = "unhealthy";
			expect(
				await Effect.runPromise(
					harness.client.ProbePreviewTarget({ target_id: "preview-target" }),
				),
			).toMatchObject({
				health: { status: "unhealthy", status_code: 503 },
				state: "unhealthy",
			});
			health_failure = true;
			await expect(
				Effect.runPromise(
					harness.client.ProbePreviewTarget({ target_id: "preview-target" }),
				),
			).rejects.toMatchObject({ protocol_code: "preview.health_unavailable" });
			expect(
				await Effect.runPromise(
					harness.client.GetPreviewTarget({ target_id: "preview-target" }),
				),
			).toMatchObject({ last_error: "health_probe_unavailable", state: "unhealthy" });
			expect(
				await Effect.runPromise(
					harness.client.SetPreviewTargetState({
						state: "stopped",
						target_id: "preview-target",
					}),
				),
			).toMatchObject({ id: "preview-target", state: "stopped" });
			expect(
				await Effect.runPromise(
					harness.client.ListPreviewTargets({ workspace_id: "preview-workspace" }),
				),
			).toMatchObject([{ id: "preview-target", state: "stopped" }]);
			expect(
				await Effect.runPromise(
					harness.client.RemovePreviewTarget({ target_id: "preview-target" }),
				),
			).toMatchObject({ id: "preview-target", state: "stopped" });
			expect(
				await Effect.runPromise(
					harness.client.ListPreviewTargets({ workspace_id: "preview-workspace" }),
				),
			).toEqual([]);
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	});
});
