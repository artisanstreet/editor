import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { createHash } from "node:crypto";

import { Effect, Layer, ManagedRuntime, Schema } from "effect";
import { afterEach, describe, expect, it } from "vitest";
import { MarketplaceLedgerEvent, type CapabilityDetail } from "@artisan/protocol";

import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import {
	JournalEvents,
	MarketplaceCapabilityArtifacts,
	MarketplaceCapabilityOperations,
} from "../../modules/backend/src/persistence/schema";
import {
	CapabilityRepository,
	CapabilityRepositoryLive,
} from "../../modules/backend/src/marketplace/capabilities/capability-repository";
import {
	CapabilityService,
	CapabilityOAuthLifecycle,
	CapabilityOAuthLifecycleLive,
	CapabilityServiceLive,
} from "../../modules/backend/src/marketplace/capabilities/capability-service";

import { CapabilityTransportRegistry } from "../../modules/backend/src/marketplace/capabilities/mcp-transport";
import { McpTransportError } from "../../modules/backend/src/marketplace/capabilities/mcp-transport";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";
import {
	make_oauth_layer,
	OAuthError,
	OAuthAdapter,
} from "../../modules/backend/src/marketplace/capabilities/oauth";

const InvocationFingerprint = (arguments_json: string) =>
	createHash("sha256")
		.update(
			JSON.stringify({
				arguments_json,
				capability_id: "capability_a",
				scope: { kind: "global" },
				tool_name: "write",
			}),
		)
		.digest("hex");

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: Array<string> = [];

const MakePath = async () => {
	const directory = await mkdtemp(join(tmpdir(), "artisan-capability-repository-"));
	directories.push(directory);
	return join(directory, "artisan.db");
};

let identifier = 0;
const MetadataLive = Layer.succeed(RuntimeMetadata, {
	instance_id: "capability_repository_test",
	MakeId: (prefix) => Effect.sync(() => `${prefix}_test_${++identifier}`),
	Now: Effect.succeed("2026-07-18T12:00:00.000Z"),
});
const Connect = (
	service: typeof CapabilityService.Service,
	detail: CapabilityDetail,
	operation_id: string,
	request_fingerprint: string,
	approved = true,
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
			approved,
		});
	});

const MakeRuntime = (database_path: string) =>
	ManagedRuntime.make(
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

const capability_detail = {
	auth: { kind: "none" as const },
	compatibility: [],
	display_name: "Capability A",
	enabled: true,
	health: { status: "unknown" as const },
	id: "capability_a",
	lifecycle: "awaiting_approval" as const,
	permissions: [],
	policy: [{ approval: "never" as const, enabled: true, name: "read" }],
	resources: [],
	scope: { kind: "global" as const },
	status: "awaiting_approval" as const,
	source: { kind: "catalog" as const, locator: "capability-a" },
	sync: [],
	tools: [{ name: "read" }],
	transport: { args: [], command: "fake", kind: "stdio" as const, startup_timeout_ms: 100 },
	trust: "verified" as const,
};

const oauth_capability_detail: CapabilityDetail = {
	...capability_detail,
	auth: {
		authorization_url: "https://auth.example.test/authorize",
		kind: "oauth",
		provider: "example",
		scopes: [],
		token_status: "not_started",
	},
};

afterEach(async () => {
	await Promise.all(
		directories.splice(0).map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("CapabilityRepository connection admission", () => {
	it("persists an exact request once and rejects a changed retry", async () => {
		const runtime = MakeRuntime(await MakePath());
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* CapabilityRepository;
					const input = {
						capability_id: "capability_a",
						detail: capability_detail,
						operation_id: "operation_a",
						request_fingerprint: "fingerprint_a",
					};
					const accepted = yield* repository.RecordConnectRequest(input);
					const duplicate = yield* repository.RecordConnectRequest(input);
					const conflict = yield* Effect.exit(
						repository.RecordConnectRequest({
							...input,
							request_fingerprint: "fingerprint_b",
						}),
					);
					return { accepted, conflict, duplicate };
				}),
			);
			expect(result.accepted).toBe("accepted");
			expect(result.duplicate).toBe("duplicate");
			expect(result.conflict._tag).toBe("Failure");
		} finally {
			await runtime.dispose();
		}
	});

	it("makes a denial terminal before any capability is created", async () => {
		const runtime = MakeRuntime(await MakePath());
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* CapabilityRepository;
					yield* repository.RecordConnectRequest({
						approval_id: "approval_a",
						approval_fingerprint: "preview_a",
						capability_id: "capability_a",
						detail: capability_detail,
						operation_id: "operation_a",
						request_fingerprint: "fingerprint_a",
					});
					const decision = yield* repository.DecideConnect({
						approval_id: "approval_a",
						approval_fingerprint: "preview_a",
						approved: false,
					});
					return { decision, summaries: yield* repository.ReadSummaries };
				}),
			);
			expect(result).toEqual({ decision: "denied", summaries: [] });
		} finally {
			await runtime.dispose();
		}
	});

	it("keeps a claimed uninstall recoverable without emitting an uninstall event twice", async () => {
		const runtime = MakeRuntime(await MakePath());
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* CapabilityRepository;
					yield* repository.RecordConnectRequest({
						approval_id: "approval_uninstall",
						approval_fingerprint: "preview_uninstall",
						capability_id: capability_detail.id,
						detail: capability_detail,
						operation_id: "uninstall_seed",
						request_fingerprint: "uninstall_seed",
					});
					yield* repository.DecideConnect({
						approval_id: "approval_uninstall",
						approval_fingerprint: "preview_uninstall",
						approved: true,
					});
					yield* repository.ClaimConnect("uninstall_seed");
					yield* repository.Create({
						detail: capability_detail,
						operation_id: "uninstall_seed",
						request_fingerprint: "uninstall_seed",
					});
					yield* repository.RecordUninstall({
						capability_id: capability_detail.id,
						operation_id: "uninstall_operation",
					});
					const claimed = yield* repository.ClaimUninstall("uninstall_operation");
					const recovered = yield* repository.ClaimUninstall("uninstall_operation");
					const database = yield* Database;
					const before = yield* database.client.select().from(JournalEvents);
					yield* repository.CompleteUninstall("uninstall_operation");
					yield* repository.CompleteUninstall("uninstall_operation");
					const after = yield* database.client.select().from(JournalEvents);
					return { after, before, claimed, recovered };
				}),
			);
			expect(result.claimed).toBe("claimed");
			expect(result.recovered).toBe("closing");
			expect(result.before).toHaveLength(1);
			expect(result.after).toHaveLength(2);
		} finally {
			await runtime.dispose();
		}
	});

	it("does not connect when the persisted approval decision is denied", async () => {
		let connect_count = 0;
		const runtime = ManagedRuntime.make(
			CapabilityServiceLive.pipe(
				Layer.provide(CapabilityRepositoryLive),
				Layer.provideMerge(
					Layer.mergeAll(
						make_database_layer({
							database_path: await MakePath(),
							migrations_path,
						}),
						JournalNotifierLive,
						MetadataLive,
						Layer.succeed(CapabilityTransportRegistry, {
							Connect: () =>
								Effect.sync(() => {
									connect_count += 1;
									return {
										CallTool: () => Effect.succeed({}),
										Close: Effect.void,
										Health: Effect.succeed("connected" as const),
										Initialize: Effect.succeed({
											protocol_version: "1",
											server_name: "fake",
										}),
										ListResources: Effect.succeed([]),
										ListTools: Effect.succeed([]),
									};
								}),
						}),
					),
				),
			),
		);
		try {
			const exit = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						return yield* Connect(
							yield* CapabilityService,
							capability_detail,
							"operation_denied",
							"fingerprint_denied",
							false,
						);
					}),
				).pipe(Effect.exit),
			);
			expect(exit._tag).toBe("Failure");
			expect(connect_count).toBe(0);
		} finally {
			await runtime.dispose();
		}
	});

	it("connects an approved request once and exposes its canonical detail", async () => {
		let connect_count = 0;
		const runtime = ManagedRuntime.make(
			CapabilityServiceLive.pipe(
				Layer.provide(CapabilityRepositoryLive),
				Layer.provideMerge(
					Layer.mergeAll(
						make_database_layer({ database_path: await MakePath(), migrations_path }),
						JournalNotifierLive,
						MetadataLive,
						Layer.succeed(CapabilityTransportRegistry, {
							Connect: () =>
								Effect.sync(() => {
									connect_count += 1;
									return {
										CallTool: () => Effect.succeed({}),
										Close: Effect.void,
										Health: Effect.succeed("connected" as const),
										Initialize: Effect.succeed({
											protocol_version: "1",
											server_name: "fake",
										}),
										ListResources: Effect.succeed([]),
										ListTools: Effect.succeed([]),
									};
								}),
						}),
					),
				),
			),
		);
		try {
			const detail = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const service = yield* CapabilityService;
						return yield* Connect(
							service,
							capability_detail,
							"operation_approved",
							"fingerprint_approved",
						);
					}),
				),
			);
			const duplicate = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const service = yield* CapabilityService;
						return yield* Connect(
							service,
							capability_detail,
							"operation_approved",
							"fingerprint_approved",
						);
					}),
				),
			);
			await runtime.runPromise(
				Effect.gen(function* () {
					const service = yield* CapabilityService;
					yield* service.Uninstall({
						capability_id: "capability_a",
						operation_id: "operation_uninstall",
					});
				}),
			);
			expect(connect_count).toBe(1);
			expect(detail).toMatchObject({ health: { status: "healthy" }, lifecycle: "connected" });
			expect(duplicate.id).toBe("capability_a");
		} finally {
			await runtime.dispose();
		}
	});

	it("defaults unconfigured tools to approval and records only an artifact reference after invocation", async () => {
		let calls = 0;
		let fail_invocation = false;
		const runtime = ManagedRuntime.make(
			CapabilityServiceLive.pipe(
				Layer.provide(CapabilityRepositoryLive),
				Layer.provideMerge(
					Layer.mergeAll(
						make_database_layer({ database_path: await MakePath(), migrations_path }),
						JournalNotifierLive,
						MetadataLive,
						Layer.succeed(CapabilityTransportRegistry, {
							Connect: () =>
								Effect.succeed({
									CallTool: () =>
										Effect.sync(() => {
											calls += 1;
											return fail_invocation;
										}).pipe(
											Effect.flatMap((failed) =>
												failed
													? Effect.fail(
															new McpTransportError({
																operation: "call_tool",
																state: "crashed",
															}),
														)
													: Effect.succeed({}),
											),
										),
									Close: Effect.void,
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
			const result = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const service = yield* CapabilityService;
						const detail = {
							...capability_detail,
							tools: [{ name: "read" }, { name: "write" }],
						};
						yield* Connect(
							service,
							detail,
							"operation_invoke_connect",
							"fingerprint_invoke_connect",
						);
						const Request = (operation_id: string, approval_id: string) =>
							service.RequestInvocation({
								approval_id,
								arguments_json: "{}",
								capability_id: "capability_a",
								intent_fingerprint: InvocationFingerprint("{}"),
								operation_id,
								requested_by: "user",
								scope: { kind: "global" },
								tool_name: "write",
							});
						const Decide = (approval_id: string, approved: boolean) =>
							service.DecideInvocation({
								approval_id,
								approved,
								arguments_json: "{}",
								capability_id: "capability_a",
								intent_fingerprint: InvocationFingerprint("{}"),
								scope: { kind: "global" },
								tool_name: "write",
							});
						yield* Request("operation_invoke_denied", "approval_invoke_denied");
						const denied = yield* Decide("approval_invoke_denied", false);
						yield* Request("operation_invoke_completed", "approval_invoke_completed");
						const completed = yield* Decide("approval_invoke_completed", true);
						const duplicate = yield* Decide("approval_invoke_completed", true);
						const conflict = yield* service
							.DecideInvocation({
								approval_id: "approval_invoke_completed",
								approved: true,
								arguments_json: '{"changed":true}',
								capability_id: "capability_a",
								intent_fingerprint: InvocationFingerprint("{}"),
								scope: { kind: "global" },
								tool_name: "write",
							})
							.pipe(Effect.exit);
						yield* Request("operation_invoke_crashed", "approval_invoke_crashed");
						fail_invocation = true;
						const crashed = yield* service
							.DecideInvocation({
								approval_id: "approval_invoke_crashed",
								approved: true,
								arguments_json: "{}",
								capability_id: "capability_a",
								intent_fingerprint: InvocationFingerprint("{}"),
								scope: { kind: "global" },
								tool_name: "write",
							})
							.pipe(Effect.exit);
						fail_invocation = false;
						const recovered = yield* Decide("approval_invoke_crashed", true).pipe(
							Effect.exit,
						);
						const database = yield* Database;
						const [artifact_row] = yield* database.client
							.select()
							.from(MarketplaceCapabilityArtifacts);
						const artifact = yield* Schema.decodeUnknownEffect(
							Schema.UnknownFromJsonString,
						)(artifact_row!.result_json);
						const operations = yield* database.client
							.select()
							.from(MarketplaceCapabilityOperations);
						const events = yield* database.client.select().from(JournalEvents);
						const lifecycle_events = yield* Effect.forEach(events, (event) =>
							Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
								event.payload_json,
							).pipe(
								Effect.flatMap(Schema.decodeUnknownEffect(MarketplaceLedgerEvent)),
								Effect.option,
							),
						);
						return {
							completed,
							artifact,
							conflict,
							crashed,
							denied,
							duplicate,
							lifecycle_events,
							operations,
							recovered,
						};
					}),
				),
			);
			expect(result.denied.status).toBe("denied");
			expect(result.completed.approval_required).toBe(true);
			expect(result.completed.result_artifact_id).toMatch(/^artifact_/);
			expect(result.duplicate).toEqual(result.completed);
			expect(result.conflict._tag).toBe("Failure");
			expect(result.crashed._tag).toBe("Success");
			if (result.crashed._tag === "Success")
				expect(result.crashed.value.status).toBe("failed");
			expect(result.recovered._tag).toBe("Failure");
			expect(result.artifact).toEqual({});
			expect(
				result.operations.find(
					(operation) => operation.operation_id === "operation_invoke_crashed",
				)?.state,
			).toBe("failed");
			expect(calls).toBe(2);
			expect(
				result.lifecycle_events.some(
					(event) =>
						event._tag === "Some" &&
						event.value.artifact_id === result.completed.result_artifact_id,
				),
			).toBe(true);
		} finally {
			await runtime.dispose();
		}
	});
});

describe("Capability OAuth lifecycle", () => {
	it("journals lifecycle status without persisting callback or token material", async () => {
		const database_path = await MakePath();
		const repository_runtime = MakeRuntime(database_path);
		const completed_token_reference = { provider: "vault", secret_id: "completed" };
		const refreshed_token_reference = { provider: "vault", secret_id: "refreshed" };
		let begins = 0;
		let completes = 0;
		let refreshes = 0;
		let revokes = 0;
		try {
			await repository_runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* CapabilityRepository;
					yield* repository.RecordConnectRequest({
						approval_id: "approval_oauth",
						approval_fingerprint: "preview_oauth",
						capability_id: oauth_capability_detail.id,
						detail: oauth_capability_detail,
						operation_id: "oauth_connect",
						request_fingerprint: "oauth_connect_fingerprint",
					});
					yield* repository.DecideConnect({
						approval_id: "approval_oauth",
						approval_fingerprint: "preview_oauth",
						approved: true,
					});
					yield* repository.ClaimConnect("oauth_connect");
					yield* repository.Create({
						detail: oauth_capability_detail,
						operation_id: "oauth_connect",
						request_fingerprint: "oauth_connect_fingerprint",
					});
				}),
			);
			const oauth_adapter = Layer.succeed(OAuthAdapter, {
				Begin: () =>
					Effect.sync(() => {
						begins += 1;
						return {
							authorization_url: "https://example.test/login",
							state: "browser-state",
						};
					}),
				Complete: (input) =>
					Effect.gen(function* () {
						completes += 1;
						if (input.callback_reference === "wrong-callback-reference")
							return yield* new OAuthError({ operation: "complete" });
						return {
							capability_id: "capability_a",
							secret_reference: completed_token_reference,
							state: "active" as const,
						};
					}),
				Refresh: () =>
					Effect.gen(function* () {
						refreshes += 1;
						if (refreshes === 2) return yield* new OAuthError({ operation: "refresh" });
						return {
							capability_id: "capability_a",
							secret_reference: refreshed_token_reference,
							state: "active" as const,
						};
					}),
				Revoke: () =>
					Effect.sync(() => {
						revokes += 1;
					}),
				Status: () =>
					Effect.succeed({ capability_id: "capability_a", state: "absent" as const }),
			});
			const lifecycle_runtime = ManagedRuntime.make(
				CapabilityOAuthLifecycleLive.pipe(
					Layer.provide(CapabilityRepositoryLive),
					Layer.provide(make_oauth_layer.pipe(Layer.provide(oauth_adapter))),
					Layer.provideMerge(
						Layer.mergeAll(
							make_database_layer({ database_path, migrations_path }),
							JournalNotifierLive,
							MetadataLive,
						),
					),
				),
			);
			try {
				await lifecycle_runtime.runPromise(Effect.void);
				expect({ begins, completes, refreshes, revokes }).toEqual({
					begins: 0,
					completes: 0,
					refreshes: 0,
					revokes: 0,
				});
				const persisted = await lifecycle_runtime.runPromise(
					Effect.gen(function* () {
						const lifecycle = yield* CapabilityOAuthLifecycle;
						const begin = yield* lifecycle.Begin({
							authorization_url: "https://example.test/login",
							capability_id: "capability_a",
							operation_id: "oauth_begin",
							scopes: [],
						});
						expect(begin._tag).toBe("started");
						const begin_retry = yield* lifecycle.Begin({
							authorization_url: "https://example.test/login",
							capability_id: "capability_a",
							operation_id: "oauth_begin",
							scopes: [],
						});
						expect(begin_retry).toEqual({
							_tag: "started",
							authorization_url: "https://example.test/login",
							state: "browser-state",
						});
						const mismatch = yield* lifecycle
							.Complete({
								capability_id: "capability_a",
								callback_reference: "wrong-callback-reference",
								operation_id: "oauth_complete_mismatch",
							})
							.pipe(Effect.exit);
						expect(mismatch._tag).toBe("Failure");
						yield* lifecycle.Complete({
							capability_id: "capability_a",
							callback_reference: "callback-secret",
							operation_id: "oauth_complete",
						});
						const complete_retry = yield* lifecycle.Complete({
							capability_id: "capability_a",
							callback_reference: "callback-secret",
							operation_id: "oauth_complete",
						});
						expect(complete_retry).toEqual({
							capability_id: "capability_a",
							secret_reference: completed_token_reference,
							state: "active",
						});
						yield* lifecycle.Refresh({
							capability_id: "capability_a",
							operation_id: "oauth_refresh",
						});
						const refresh_retry = yield* lifecycle.Refresh({
							capability_id: "capability_a",
							operation_id: "oauth_refresh",
						});
						expect(refresh_retry).toEqual({
							capability_id: "capability_a",
							secret_reference: refreshed_token_reference,
							state: "active",
						});
						const crashed = yield* lifecycle
							.Refresh({
								capability_id: "capability_a",
								operation_id: "oauth_refresh_crash",
							})
							.pipe(Effect.exit);
						const crashed_retry = yield* lifecycle
							.Refresh({
								capability_id: "capability_a",
								operation_id: "oauth_refresh_crash",
							})
							.pipe(Effect.exit);
						expect([crashed._tag, crashed_retry._tag]).toEqual(["Failure", "Failure"]);
						yield* lifecycle.Revoke({
							capability_id: "capability_a",
							operation_id: "oauth_revoke",
						});
						yield* lifecycle.Revoke({
							capability_id: "capability_a",
							operation_id: "oauth_revoke",
						});
						const database = yield* Database;
						return {
							journal: yield* database.client
								.select({ payload_json: JournalEvents.payload_json })
								.from(JournalEvents),
							operations: yield* database.client
								.select({
									preview_json: MarketplaceCapabilityOperations.preview_json,
									request_fingerprint:
										MarketplaceCapabilityOperations.request_fingerprint,
								})
								.from(MarketplaceCapabilityOperations),
						};
					}),
				);
				expect({ begins, completes, refreshes, revokes }).toEqual({
					begins: 1,
					completes: 2,
					refreshes: 2,
					revokes: 1,
				});
				expect(persisted.journal).toHaveLength(5);
				const durable_text = JSON.stringify(persisted);
				expect(durable_text).not.toContain("callback-secret");
				expect(durable_text).not.toContain("wrong-callback-reference");
				const detail = await repository_runtime.runPromise(
					Effect.gen(function* () {
						return yield* (yield* CapabilityRepository).ReadDetail("capability_a");
					}),
				);
				expect(detail.auth).toMatchObject({
					kind: "oauth",
					token_status: "not_started",
				});
				if (detail.auth.kind === "oauth") expect(detail.auth.token_ref).toBeUndefined();
			} finally {
				await lifecycle_runtime.dispose();
			}
		} finally {
			await repository_runtime.dispose();
		}
	});
});
