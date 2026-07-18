import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { createHash } from "node:crypto";

import { Effect, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";
import type { CapabilityDetail } from "@artisan/protocol";

import { make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import {
	CapabilityRepository,
	CapabilityRepositoryLive,
} from "../../modules/backend/src/marketplace/capabilities/capability-repository";
import {
	CapabilityService,
	CapabilityServiceLive,
} from "../../modules/backend/src/marketplace/capabilities/capability-service";
import {
	CapabilityTransportRegistry,
	make_capability_transport_registry_layer,
} from "../../modules/backend/src/marketplace/capabilities/mcp-transport";
import {
	CapabilityMirrorService,
	CapabilityMirrorServiceLive,
	CapabilityProviderMirror,
} from "../../modules/backend/src/marketplace/capabilities/provider-mirrors";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";

const DriftFingerprint = (capability_id: string, observed_revision: string) =>
	createHash("sha256")
		.update(
			JSON.stringify({
				capability_id,
				engine_id: "codex",
				observed_revision,
				scope: { kind: "global" },
			}),
		)
		.digest("hex");

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: Array<string> = [];
let ids = 0;
const MakePath = async () => {
	const directory = await mkdtemp(join(tmpdir(), "artisan-capability-service-"));
	directories.push(directory);
	return join(directory, "artisan.db");
};
const MetadataLive = Layer.succeed(RuntimeMetadata, {
	instance_id: "capability_service_test",
	MakeId: (prefix) => Effect.sync(() => `${prefix}_capability_${++ids}`),
	Now: Effect.succeed("2026-07-18T12:00:00.000Z"),
});
const Connect = (
	service: typeof CapabilityService.Service,
	detail: CapabilityDetail,
	operation_id: string,
	request_fingerprint: string,
) =>
	Effect.gen(function* () {
		const preview = yield* service.Preview(detail);
		const approval_id = `${operation_id}:approval`;
		yield* service.RequestConnect({
			approval_id,
			detail,
			operation_id,
			preview_fingerprint: preview.preview_fingerprint,
			request_fingerprint,
		});
		return yield* service.DecideConnect({
			approval_fingerprint: preview.preview_fingerprint,
			approval_id,
			approved: true,
		});
	});
const detail = {
	auth: { kind: "none" as const },
	compatibility: [],
	display_name: "Recovery",
	enabled: true,
	health: { status: "unknown" as const },
	id: "capability_recovery",
	lifecycle: "awaiting_approval" as const,
	permissions: [],
	policy: [],
	resources: [],
	scope: { kind: "global" as const },
	status: "awaiting_approval" as const,
	source: { kind: "catalog" as const, locator: "recovery" },
	sync: [],
	tools: [],
	transport: { args: [], command: "fake", kind: "stdio" as const, startup_timeout_ms: 100 },
	trust: "verified" as const,
};

afterEach(async () => {
	await Promise.all(
		directories.splice(0).map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("Capability service recovery", () => {
	it("reuses durable approval for exact session actions and keeps enable inert", async () => {
		const database_path = await MakePath();
		let connects = 0;
		let closes = 0;
		const runtime = ManagedRuntime.make(
			CapabilityServiceLive.pipe(
				Layer.provide(CapabilityRepositoryLive),
				Layer.provideMerge(
					Layer.mergeAll(
						make_database_layer({ database_path, migrations_path }),
						JournalNotifierLive,
						MetadataLive,
						Layer.succeed(CapabilityTransportRegistry, {
							Connect: () =>
								Effect.sync(() => {
									connects += 1;
									return {
										CallTool: () => Effect.succeed({}),
										Close: Effect.sync(() => {
											closes += 1;
										}),
										Health: Effect.succeed("connected" as const),
										Initialize: Effect.succeed({
											instructions: "Server guidance",
											protocol_version: "2025-06-18",
											server_name: "refreshed",
											server_version: "2.0.0",
										}),
										ListResources: Effect.succeed([
											{ name: "Guide", uri: "file:///refreshed" },
										]),
										ListTools: Effect.succeed([
											{ input_schema: {}, name: "refreshed_tool" },
										]),
									};
								}),
						}),
					),
				),
			),
		);
		try {
			const result = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const service = yield* CapabilityService;
						yield* Connect(service, detail, "session_connect", "session_connect");
						yield* service.SessionAction({
							action: "reconnect",
							capability_id: detail.id,
							operation_id: "session_reconnect",
						});
						const restarted = yield* service.SessionAction({
							action: "restart",
							capability_id: detail.id,
							operation_id: "session_restart",
						});
						const retry = yield* service.SessionAction({
							action: "restart",
							capability_id: detail.id,
							operation_id: "session_restart",
						});
						yield* service.Disable({
							capability_id: detail.id,
							operation_id: "session_disable",
						});
						yield* service.Enable({
							capability_id: detail.id,
							operation_id: "session_enable",
						});
						yield* service.Remove({
							capability_id: detail.id,
							operation_id: "session_remove",
						});
						const removed_reconnect = yield* Effect.exit(
							service.SessionAction({
								action: "reconnect",
								capability_id: detail.id,
								operation_id: "session_removed_reconnect",
							}),
						);
						const removed_enable = yield* Effect.exit(
							service.Enable({
								capability_id: detail.id,
								operation_id: "session_removed_enable",
							}),
						);
						return { removed_enable, removed_reconnect, restarted, retry };
					}),
				),
			);
			expect(connects).toBe(2);
			expect(closes).toBe(2);
			expect(result.retry).toEqual(result.restarted);
			expect(result.removed_reconnect._tag).toBe("Failure");
			expect(result.removed_enable._tag).toBe("Failure");
			expect(result.restarted.server_instructions).toBe("Server guidance");
			expect(result.restarted.server_metadata).toEqual({
				protocol_version: "2025-06-18",
				server_name: "refreshed",
				server_version: "2.0.0",
			});
			expect(result.restarted.tools).toEqual([{ input_schema: {}, name: "refreshed_tool" }]);
			expect(result.restarted.resources).toEqual([{ uri: "file:///refreshed" }]);
		} finally {
			await runtime.dispose();
		}
	});
	it("acquires the canonical selector inertly and selects only the reviewed transport kind", async () => {
		const selected: Array<string> = [];
		const session = {
			CallTool: () => Effect.succeed({}),
			Close: Effect.void,
			Health: Effect.succeed("connected" as const),
			Initialize: Effect.succeed({ protocol_version: "1", server_name: "fake" }),
			ListResources: Effect.succeed([]),
			ListTools: Effect.succeed([]),
		};
		const layer = make_capability_transport_registry_layer({
			http: { Connect: () => Effect.sync(() => (selected.push("http"), session)) },
			stdio: { Connect: () => Effect.sync(() => (selected.push("stdio"), session)) },
		});
		const registry = await Effect.runPromise(
			Effect.gen(function* () {
				return yield* CapabilityTransportRegistry;
			}).pipe(Effect.provide(layer)),
		);
		expect(selected).toEqual([]);
		await Effect.runPromise(
			Effect.scoped(
				registry.Connect(detail).pipe(
					Effect.andThen(
						registry.Connect({
							auth: detail.auth,
							transport: { kind: "streamable_http", url: "https://example.test/mcp" },
						}),
					),
				),
			),
		);
		expect(selected).toEqual(["stdio", "http"]);
	});

	it("persists validated MCP discovery without credential material", async () => {
		const database_path = await MakePath();
		const runtime = ManagedRuntime.make(
			CapabilityServiceLive.pipe(
				Layer.provide(CapabilityRepositoryLive),
				Layer.provideMerge(
					Layer.mergeAll(
						make_database_layer({ database_path, migrations_path }),
						JournalNotifierLive,
						MetadataLive,
						Layer.succeed(CapabilityTransportRegistry, {
							Connect: () =>
								Effect.succeed({
									CallTool: () => Effect.succeed({}),
									Close: Effect.void,
									Health: Effect.succeed("connected" as const),
									Initialize: Effect.succeed({
										protocol_version: "1",
										server_name: "discovered",
									}),
									ListResources: Effect.succeed([
										{
											uri: "file:///guide",
											name: "Guide",
											description: "Docs",
										},
									]),
									ListTools: Effect.succeed([
										{ name: "search", description: "Search", input_schema: {} },
									]),
								}),
						}),
					),
				),
			),
		);
		try {
			const discovered = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const service = yield* CapabilityService;
						return yield* Connect(service, detail, "discover_connect", "discover");
					}),
				),
			);
			expect(discovered.tools).toEqual([
				{ description: "Search", input_schema: {}, name: "search" },
			]);
			expect(discovered.resources).toEqual([{ uri: "file:///guide", description: "Docs" }]);
			expect(discovered.permissions).toEqual([
				{ kind: "process", description: "Start the configured MCP server process" },
			]);
			expect(discovered.health.status).toBe("healthy");
			expect(JSON.stringify(discovered)).not.toContain("credential");
		} finally {
			await runtime.dispose();
		}
	});

	it("never repeats a claimed uninstall close and finalizes the successful close once", async () => {
		const database_path = await MakePath();
		let closes = 0;
		const runtime = ManagedRuntime.make(
			CapabilityServiceLive.pipe(
				Layer.provide(CapabilityRepositoryLive),
				Layer.provideMerge(
					Layer.mergeAll(
						make_database_layer({ database_path, migrations_path }),
						JournalNotifierLive,
						MetadataLive,
						Layer.succeed(CapabilityTransportRegistry, {
							Connect: () =>
								Effect.succeed({
									CallTool: () => Effect.succeed({}),
									Close: Effect.sync(() => {
										closes += 1;
									}),
									Health: Effect.succeed("connected" as const),
									Initialize: Effect.succeed({
										protocol_version: "1",
										server_name: "fake",
									}),
									ListResources: Effect.succeed([]),
									ListTools: Effect.succeed([]),
								}),
						}),
					),
				),
			),
		);
		try {
			await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const service = yield* CapabilityService;
						yield* Connect(service, detail, "uninstall_connect", "uninstall_connect");
						yield* service.Uninstall({
							capability_id: detail.id,
							operation_id: "uninstall_ok",
						});
						yield* service.Uninstall({
							capability_id: detail.id,
							operation_id: "uninstall_ok",
						});
					}),
				),
			);
			expect(closes).toBe(1);
		} finally {
			await runtime.dispose();
		}
	});

	it("writes provider drift overwrite once while import and ignore remain write-free", async () => {
		const database_path = await MakePath();
		const repository_runtime = ManagedRuntime.make(
			CapabilityRepositoryLive.pipe(
				Layer.provideMerge(
					Layer.mergeAll(
						make_database_layer({ database_path, migrations_path }),
						JournalNotifierLive,
						MetadataLive,
					),
				),
			),
		);
		let writes = 0;
		try {
			await repository_runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* CapabilityRepository;
					yield* repository.RecordConnectRequest({
						approval_id: "approval_drift",
						approval_fingerprint: "preview_drift",
						capability_id: detail.id,
						detail,
						operation_id: "drift_connect",
						request_fingerprint: "drift_connect",
					});
					yield* repository.DecideConnect({
						approval_id: "approval_drift",
						approval_fingerprint: "preview_drift",
						approved: true,
					});
					yield* repository.ClaimConnect("drift_connect");
					yield* repository.Create({
						detail,
						operation_id: "drift_connect",
						request_fingerprint: "drift_connect",
					});
				}),
			);
			const mirror_runtime = ManagedRuntime.make(
				CapabilityMirrorServiceLive.pipe(
					Layer.provide(CapabilityRepositoryLive),
					Layer.provideMerge(
						Layer.mergeAll(
							make_database_layer({ database_path, migrations_path }),
							JournalNotifierLive,
							MetadataLive,
							Layer.succeed(CapabilityProviderMirror, {
								Sync: () => Effect.die("unused"),
								ResolveDrift: ({ engine_id, observed_revision }) =>
									Effect.sync(() => {
										writes += 1;
										return {
											engine_id,
											observed_revision,
											status: "synced" as const,
											updated_at: "2026-07-18T12:00:00.000Z",
										};
									}),
							}),
						),
					),
				),
			);
			try {
				const result = await mirror_runtime.runPromise(
					Effect.gen(function* () {
						const mirrors = yield* CapabilityMirrorService;
						const overwrite_fingerprint = DriftFingerprint(detail.id, "overwrite");
						yield* mirrors.ResolveDrift({
							action: "ignore",
							capability_id: detail.id,
							engine_id: "codex",
							observed_revision: "ignore",
							operation_id: "drift_ignore",
						});
						yield* mirrors.ResolveDrift({
							action: "import",
							capability_id: detail.id,
							engine_id: "codex",
							observed_revision: "import",
							operation_id: "drift_import",
						});
						yield* mirrors.RequestOverwrite({
							approval_fingerprint: overwrite_fingerprint,
							approval_id: "drift_approval",
							capability_id: detail.id,
							engine_id: "codex",
							observed_revision: "overwrite",
							operation_id: "drift_overwrite",
							scope: { kind: "global" },
						});
						const first = yield* mirrors.DecideOverwrite({
							approval_fingerprint: overwrite_fingerprint,
							approval_id: "drift_approval",
							approved: true,
							capability_id: detail.id,
							engine_id: "codex",
							observed_revision: "overwrite",
							scope: { kind: "global" },
						});
						const retry = yield* mirrors.DecideOverwrite({
							approval_fingerprint: overwrite_fingerprint,
							approval_id: "drift_approval",
							approved: true,
							capability_id: detail.id,
							engine_id: "codex",
							observed_revision: "overwrite",
							scope: { kind: "global" },
						});
						return { first, retry };
					}),
				);
				expect(writes).toBe(1);
				expect(result.retry).toEqual(result.first);
			} finally {
				await mirror_runtime.dispose();
			}
		} finally {
			await repository_runtime.dispose();
		}
	});

	it("does not reconnect a persisted capability after runtime restart", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-capability-service-"));
		directories.push(directory);
		const database_path = join(directory, "artisan.db");
		let connects = 0;
		const transport = Layer.succeed(CapabilityTransportRegistry, {
			Connect: () =>
				Effect.sync(() => {
					connects += 1;
					return {
						CallTool: () => Effect.succeed({}),
						Close: Effect.void,
						Health: Effect.succeed("connected" as const),
						Initialize: Effect.succeed({ protocol_version: "1", server_name: "fake" }),
						ListResources: Effect.succeed([]),
						ListTools: Effect.succeed([]),
					};
				}),
		});
		const MakeRuntime = () =>
			ManagedRuntime.make(
				CapabilityServiceLive.pipe(
					Layer.provide(CapabilityRepositoryLive),
					Layer.provideMerge(
						Layer.mergeAll(
							make_database_layer({ database_path, migrations_path }),
							JournalNotifierLive,
							MetadataLive,
							transport,
						),
					),
				),
			);
		const first = MakeRuntime();
		try {
			const connected = await first.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const service = yield* CapabilityService;
						return yield* Connect(
							service,
							detail,
							"recovery_connect",
							"recovery_fingerprint",
						);
					}),
				),
			);
			expect(connected.lifecycle).toBe("connected");
		} finally {
			await first.dispose();
		}
		const second = MakeRuntime();
		try {
			const result = await second.runPromise(
				Effect.gen(function* () {
					const service = yield* CapabilityService;
					const health = yield* service.Health({
						capability_id: detail.id,
						operation_id: "recovery_health",
					});
					const reconnected = yield* Connect(
						service,
						detail,
						"recovery_explicit_reconnect",
						"recovery_explicit_reconnect",
					);
					return { health, reconnected };
				}),
			);
			expect(connects).toBe(2);
			expect(result.health.health.status).toBe("offline");
			expect(result.health.lifecycle).toBe("stopped");
			expect(result.reconnected.lifecycle).toBe("connected");
		} finally {
			await second.dispose();
		}
	});
});
