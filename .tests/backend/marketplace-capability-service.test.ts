import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { createHash } from "node:crypto";

import { Deferred, Effect, Fiber, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";
import type { CapabilityDetail } from "@artisan/protocol";

import { make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import {
	CapabilityRepository,
	CapabilityRepositoryError,
	CapabilityRepositoryLive,
} from "../../modules/backend/src/marketplace/capabilities/repository";
import {
	CapabilityService,
	CapabilityServiceLive,
} from "../../modules/backend/src/marketplace/capabilities/service";
import {
	CapabilityTransportRegistry,
	make_capability_transport_registry_layer,
} from "../../modules/backend/src/marketplace/capabilities/mcp-transport";
import {
	CapabilityMirrorService,
	CapabilityMirrorServiceLive,
	CapabilityProviderMirror,
} from "../../modules/backend/src/marketplace/capabilities/provider-mirrors";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/metadata";

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

const RecoveryOperationId = (id: string, lifecycle: "connected" | "connecting") =>
	`startup_recovery_${createHash("sha256")
		.update(JSON.stringify({ id, lifecycle }))
		.digest("hex")
		.slice(0, 24)}`;

const MakeRecoveryHarness = (input: {
	readonly failure?: CapabilityRepositoryError;
	readonly summaries: ReadonlyArray<unknown>;
}) => {
	const recovery_started = Deferred.makeUnsafe<void>();
	const release_recovery = Deferred.makeUnsafe<void>();
	const transitions: Array<{
		readonly capability_id: string;
		readonly lifecycle?: string;
		readonly operation_id: string;
	}> = [];
	const completed_session_actions: Array<{
		readonly action: string;
		readonly detail: CapabilityDetail;
		readonly operation_id: string;
		readonly server_metadata?: Readonly<Record<string, string>>;
	}> = [];
	let connector_calls = 0;
	let detail_reads = 0;
	let initialization_calls = 0;
	let session_action_claims = 0;
	let session_action_records = 0;
	let summary_reads = 0;
	const invocation_detail = { ...detail, tools: [{ name: "read" }] };
	const repository = {
		ClaimSessionAction: () =>
			Effect.sync(() => {
				session_action_claims += 1;
				return "claimed" as const;
			}),
		CompleteSessionAction: (completion: {
			readonly action: string;
			readonly detail: CapabilityDetail;
			readonly operation_id: string;
			readonly server_metadata?: Readonly<Record<string, string>>;
		}) =>
			Effect.sync(() => {
				completed_session_actions.push(completion);
			}),
		ReadDetail: () =>
			Effect.sync(() => {
				detail_reads += 1;
				return invocation_detail;
			}),
		ReadApprovedConnect: () => Effect.succeed(invocation_detail),
		ReadSummaries: Effect.gen(function* () {
			summary_reads += 1;
			yield* Deferred.succeed(recovery_started, undefined);
			yield* Deferred.await(release_recovery);
			if (input.failure) return yield* input.failure;
			return input.summaries;
		}),
		RecordSessionAction: () =>
			Effect.sync(() => {
				session_action_records += 1;
			}),
		Transition: (transition: {
			readonly capability_id: string;
			readonly lifecycle?: string;
			readonly operation_id: string;
		}) =>
			Effect.sync(() => {
				transitions.push(transition);
			}),
	} as unknown as typeof CapabilityRepository.Service;
	const runtime = ManagedRuntime.make(
		CapabilityServiceLive.pipe(
			Layer.provideMerge(Layer.succeed(CapabilityRepository, repository)),
			Layer.provideMerge(
				Layer.succeed(CapabilityTransportRegistry, {
					Connect: () =>
						Effect.sync(() => {
							connector_calls += 1;
							return {
								CallTool: () => Effect.succeed({}),
								Close: Effect.void,
								Health: Effect.succeed("connected" as const),
								Initialize: Effect.sync(() => {
									initialization_calls += 1;
									return { protocol_version: "1", server_name: "fake" };
								}),
								ListResources: Effect.succeed([]),
								ListTools: Effect.succeed([]),
							};
						}),
				}),
			),
		),
	);
	return {
		recovery_started,
		release: Deferred.succeed(release_recovery, undefined),
		runtime,
		stats: {
			completed_session_actions: () => [...completed_session_actions],
			connector_calls: () => connector_calls,
			detail_reads: () => detail_reads,
			initialization_calls: () => initialization_calls,
			session_action_claims: () => session_action_claims,
			session_action_records: () => session_action_records,
			summary_reads: () => summary_reads,
			transitions: () => [...transitions],
		},
	};
};

const MakeReadCountHarness = () => {
	let current: CapabilityDetail = {
		...detail,
		policy: [{ approval: "never" as const, enabled: true, name: "read" }],
		tools: [{ name: "read" }],
	};
	let detail_reads = 0;
	let mutate_on_read: number | undefined;
	let mutation: CapabilityDetail | undefined;
	let tool_calls = 0;
	let recorded:
		| {
				readonly approval_fingerprint?: string;
				readonly operation_id: string;
				readonly request_fingerprint: string;
		  }
		| undefined;
	const repository = {
		ClaimConnect: () => Effect.succeed("claimed" as const),
		ClaimInvocation: () => Effect.succeed("claimed" as const),
		ClaimUninstall: () => Effect.succeed("closing" as const),
		CompleteInvocation: (input: {
			readonly approval_required: boolean;
			readonly operation_id: string;
			readonly tool_name: string;
		}) =>
			Effect.succeed({
				approval_required: input.approval_required,
				capability_id: detail.id,
				invocation_id: input.operation_id,
				status: "completed" as const,
				tool_name: input.tool_name,
			}),
		CompleteUninstall: () => Effect.void,
		Create: () => Effect.void,
		DecideConnect: () => Effect.succeed("connecting" as const),
		DecideInvocation: () => Effect.succeed("approved" as const),
		FailInvocation: (input: {
			readonly approval_required: boolean;
			readonly operation_id: string;
			readonly tool_name: string;
		}) =>
			Effect.succeed({
				approval_required: input.approval_required,
				capability_id: detail.id,
				invocation_id: input.operation_id,
				status: "failed" as const,
				tool_name: input.tool_name,
			}),
		ReadConnectApproval: () => Effect.succeed({ detail: current, operation_id: "connect" }),
		ReadDetail: () =>
			Effect.sync(() => {
				detail_reads += 1;
				if (detail_reads === mutate_on_read && mutation !== undefined) current = mutation;
				return current;
			}),
		ReadInvocationApproval: () =>
			Effect.sync(() => ({
				approval_fingerprint: recorded?.approval_fingerprint ?? "intent",
				capability_id: detail.id,
				operation_id: recorded?.operation_id ?? "approval-operation",
				request_fingerprint: recorded?.request_fingerprint ?? "request",
				tool_name: "read",
			})),
		ReadSummaries: Effect.succeed([]),
		RecordConnectRequest: () => Effect.void,
		RecordInvocation: (input: {
			readonly approval_fingerprint?: string;
			readonly operation_id: string;
			readonly request_fingerprint: string;
		}) =>
			Effect.sync(() => {
				recorded = input;
			}),
		RecordUninstall: () => Effect.void,
		Transition: () => Effect.void,
	} as unknown as typeof CapabilityRepository.Service;
	const runtime = ManagedRuntime.make(
		CapabilityServiceLive.pipe(
			Layer.provideMerge(Layer.succeed(CapabilityRepository, repository)),
			Layer.provideMerge(
				Layer.succeed(CapabilityTransportRegistry, {
					Connect: () =>
						Effect.succeed({
							CallTool: () =>
								Effect.sync(() => {
									tool_calls += 1;
									return { ok: true };
								}),
							Close: Effect.void,
							Health: Effect.succeed("connected" as const),
							Initialize: Effect.succeed({
								protocol_version: "1",
								server_name: "fake",
							}),
							ListResources: Effect.succeed([]),
							ListTools: Effect.succeed([{ input_schema: {}, name: "read" }]),
						}),
				}),
			),
		),
	);
	return {
		runtime,
		set_detail: (next: CapabilityDetail) => {
			current = next;
		},
		mutate_on_read: (read: number, next: CapabilityDetail) => {
			mutate_on_read = read;
			mutation = next;
		},
		stats: { detail_reads: () => detail_reads, tool_calls: () => tool_calls },
	};
};

afterEach(async () => {
	await Promise.all(
		directories.splice(0).map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("Capability service recovery", () => {
	it("constructs during held recovery, keeps durable reads live, and gates session work", async () => {
		const harness = MakeRecoveryHarness({
			summaries: [
				{ id: "capability_alpha", lifecycle: "connecting", status: "enabled" },
				{ id: "capability_bravo", lifecycle: "connected", status: "enabled" },
			],
		});
		try {
			const service = await harness.runtime.runPromise(
				Effect.gen(function* () {
					return yield* CapabilityService;
				}),
			);
			await Effect.runPromise(Deferred.await(harness.recovery_started));

			const preview = await harness.runtime.runPromise(service.Preview(detail));
			const durable_detail = await harness.runtime.runPromise(
				Effect.gen(function* () {
					return yield* (yield* CapabilityRepository).ReadDetail(detail.id);
				}),
			);
			const durable_detail_reads = harness.stats.detail_reads();
			const action_admitted = Deferred.makeUnsafe<void>();
			const action = await harness.runtime.runPromise(
				Effect.gen(function* () {
					const fiber = yield* Effect.gen(function* () {
						yield* Deferred.succeed(action_admitted, undefined);
						return yield* service.SessionAction({
							action: "start",
							capability_id: detail.id,
							operation_id: "held_recovery_start",
						});
					}).pipe(Effect.forkChild({ startImmediately: true }));
					yield* Deferred.await(action_admitted);
					yield* Effect.yieldNow;
					const before_release = {
						action_detail_reads: harness.stats.detail_reads() - durable_detail_reads,
						action_pending: fiber.pollUnsafe() === undefined,
						claims: harness.stats.session_action_claims(),
						completions: harness.stats.completed_session_actions().length,
						connectors: harness.stats.connector_calls(),
						detail_reads: harness.stats.detail_reads(),
						initializations: harness.stats.initialization_calls(),
						records: harness.stats.session_action_records(),
						transitions: harness.stats.transitions().length,
					};
					yield* harness.release;
					return { before_release, result: yield* Fiber.join(fiber) };
				}),
			);
			expect(preview.candidate_id).toBeDefined();
			expect(durable_detail.id).toBe(detail.id);
			expect(harness.stats.summary_reads()).toBe(1);
			expect(action.before_release).toEqual({
				action_detail_reads: 0,
				action_pending: true,
				claims: 0,
				completions: 0,
				connectors: 0,
				detail_reads: durable_detail_reads,
				initializations: 0,
				records: 0,
				transitions: 0,
			});
			expect(action.result).toMatchObject({
				health: { status: "healthy" },
				id: detail.id,
				lifecycle: "connected",
			});
			expect(harness.stats.detail_reads()).toBe(2);
			expect(harness.stats.session_action_records()).toBe(1);
			expect(harness.stats.session_action_claims()).toBe(1);
			expect(harness.stats.connector_calls()).toBe(1);
			expect(harness.stats.initialization_calls()).toBe(1);
			expect(
				harness.stats.completed_session_actions().map((completion) => ({
					action: completion.action,
					capability_id: completion.detail.id,
					health: completion.detail.health.status,
					lifecycle: completion.detail.lifecycle,
					operation_id: completion.operation_id,
					server_metadata: completion.server_metadata,
				})),
			).toEqual([
				{
					action: "start",
					capability_id: detail.id,
					health: "healthy",
					lifecycle: "connected",
					operation_id: "held_recovery_start",
					server_metadata: { protocol_version: "1", server_name: "fake" },
				},
			]);
			expect(
				harness.stats.transitions().map(({ capability_id, lifecycle, operation_id }) => ({
					capability_id,
					lifecycle,
					operation_id,
				})),
			).toEqual([
				{
					capability_id: "capability_alpha",
					lifecycle: "crashed",
					operation_id: RecoveryOperationId("capability_alpha", "connecting"),
				},
				{
					capability_id: "capability_bravo",
					lifecycle: "stopped",
					operation_id: RecoveryOperationId("capability_bravo", "connected"),
				},
			]);
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("surfaces typed recovery failure while preserving safe reads and avoiding connector work", async () => {
		const failure = new CapabilityRepositoryError({
			code: "invariant",
			message: "recovery read failed",
		});
		const harness = MakeRecoveryHarness({ failure, summaries: [] });
		try {
			const service = await harness.runtime.runPromise(
				Effect.gen(function* () {
					return yield* CapabilityService;
				}),
			);
			await Effect.runPromise(Deferred.await(harness.recovery_started));
			const preview = await harness.runtime.runPromise(service.Preview(detail));
			await Effect.runPromise(harness.release);
			await expect(
				harness.runtime.runPromise(
					service.Invoke({
						arguments_json: "{}",
						capability_id: detail.id,
						operation_id: "failed_recovery_invoke",
						scope: { kind: "global" },
						tool_name: "read",
					}),
				),
			).rejects.toMatchObject({ code: "invariant", message: "recovery read failed" });
			expect(preview.preview_fingerprint).toBeDefined();
			expect(harness.stats.detail_reads()).toBe(0);
			expect(harness.stats.connector_calls()).toBe(0);
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("interrupts held recovery on disposal without late transitions or connector work", async () => {
		const harness = MakeRecoveryHarness({
			summaries: [{ id: "capability_late", lifecycle: "connected", status: "enabled" }],
		});
		const service = await harness.runtime.runPromise(
			Effect.gen(function* () {
				return yield* CapabilityService;
			}),
		);
		await Effect.runPromise(Deferred.await(harness.recovery_started));
		const invocation = harness.runtime.runPromise(
			service.Invoke({
				arguments_json: "{}",
				capability_id: detail.id,
				operation_id: "interrupted_recovery_invoke",
				scope: { kind: "global" },
				tool_name: "read",
			}),
		);
		await harness.runtime.dispose();
		await Effect.runPromise(harness.release);
		await expect(invocation).rejects.toBeDefined();
		expect(service).toBeDefined();
		expect(harness.stats.transitions()).toEqual([]);
		expect(harness.stats.connector_calls()).toBe(0);
	});

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

	it("reuses one validated detail per command while retaining the invocation execution fence", async () => {
		const harness = MakeReadCountHarness();
		try {
			const service = await harness.runtime.runPromise(
				Effect.gen(function* () {
					return yield* CapabilityService;
				}),
			);
			await harness.runtime.runPromise(
				Effect.scoped(Connect(service, detail, "read-count-connect", "read-count-connect")),
			);
			const scope = { kind: "global" as const };
			const reads_before_direct = harness.stats.detail_reads();
			await harness.runtime.runPromise(
				service.Invoke({
					arguments_json: "{}",
					capability_id: detail.id,
					operation_id: "direct",
					scope,
					tool_name: "read",
				}),
			);
			expect(harness.stats.detail_reads() - reads_before_direct).toBe(2);

			harness.set_detail({
				...detail,
				policy: [{ approval: "always", enabled: true, name: "read" }],
				tools: [{ name: "read" }],
			});
			const intent_fingerprint = createHash("sha256")
				.update(
					JSON.stringify({
						arguments_json: "{}",
						capability_id: detail.id,
						scope,
						tool_name: "read",
					}),
				)
				.digest("hex");
			const reads_before_request = harness.stats.detail_reads();
			await harness.runtime.runPromise(
				service.RequestInvocation({
					approval_id: "approval",
					arguments_json: "{}",
					capability_id: detail.id,
					intent_fingerprint,
					operation_id: "request",
					requested_by: "user",
					scope,
					tool_name: "read",
				}),
			);
			expect(harness.stats.detail_reads() - reads_before_request).toBe(1);
			const reads_before_decision = harness.stats.detail_reads();
			await harness.runtime.runPromise(
				service.DecideInvocation({
					approval_id: "approval",
					approved: true,
					arguments_json: "{}",
					capability_id: detail.id,
					intent_fingerprint,
					scope,
					tool_name: "read",
				}),
			);
			expect(harness.stats.detail_reads() - reads_before_decision).toBe(2);

			const reads_before_mutations = harness.stats.detail_reads();
			await harness.runtime.runPromise(
				service.Enable({ capability_id: detail.id, operation_id: "enable", scope }),
			);
			await harness.runtime.runPromise(
				service.Disable({ capability_id: detail.id, operation_id: "disable", scope }),
			);
			await harness.runtime.runPromise(
				service.Remove({ capability_id: detail.id, operation_id: "remove", scope }),
			);
			expect(harness.stats.detail_reads() - reads_before_mutations).toBe(3);

			const reads_before_mismatch = harness.stats.detail_reads();
			await expect(
				harness.runtime.runPromise(
					service.Enable({
						capability_id: detail.id,
						operation_id: "mismatch",
						scope: { kind: "workspace", workspace_id: "other" },
					}),
				),
			).rejects.toMatchObject({ code: "policy_denied" });
			expect(harness.stats.detail_reads() - reads_before_mismatch).toBe(1);

			harness.set_detail({
				...detail,
				policy: [{ approval: "never", enabled: true, name: "read" }],
				tools: [{ name: "read" }],
			});
			const reads_before_fence = harness.stats.detail_reads();
			harness.mutate_on_read(reads_before_fence + 2, {
				...detail,
				enabled: false,
				policy: [{ approval: "never", enabled: true, name: "read" }],
				tools: [{ name: "read" }],
			});
			await expect(
				harness.runtime.runPromise(
					service.Invoke({
						arguments_json: "{}",
						capability_id: detail.id,
						operation_id: "fenced",
						scope,
						tool_name: "read",
					}),
				),
			).rejects.toMatchObject({ code: "disabled" });
			expect(harness.stats.detail_reads() - reads_before_fence).toBe(2);
			expect(harness.stats.tool_calls()).toBe(2);
		} finally {
			await harness.runtime.dispose();
		}
	});
});
