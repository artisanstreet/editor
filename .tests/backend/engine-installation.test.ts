import { describe, expect, it } from "@effect/vitest";
import { Effect, Layer } from "effect";

import type {
	EngineAuthenticationRequestEnvelope,
	EngineInstallRequestEnvelope,
	EngineInstallationMutationResultEnvelope,
	EngineInstallationQueryEnvelope,
	EngineInstallationQueryResultEnvelope,
	EngineRollbackRequestEnvelope,
} from "@artisan/protocol";
import {
	EngineToolchain,
	EngineToolchainBusyError,
	EngineToolchainRollbackUnavailableError,
	EngineToolchainUnknownEngineError,
	type EngineToolchainStatus,
} from "@artisan/engines";

import { MakeEngineInstallationHandler } from "../../modules/backend/src/protocol/rpc/query-handlers/engine-installation";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/metadata";

const MetadataLive = Layer.succeed(RuntimeMetadata, {
	instance_id: "backend_engine_installation",
	MakeId: (prefix) => Effect.succeed(`${prefix}_engine_installation`),
	Now: Effect.succeed("2026-08-14T12:00:00.000Z"),
});

const status = (overrides: Partial<EngineToolchainStatus> = {}): EngineToolchainStatus => ({
	activity: { _tag: "idle" },
	credentials_present: false,
	display_name: "Claude",
	engine_id: "claude",
	home_path: "C:/artisan/toolchain/claude/home",
	...overrides,
});

const Trace = {
	origin: "frontend" as const,
	protocol_version: 1 as const,
	schema_version: 1 as const,
	sent_at: "2026-08-14T12:00:00.000Z",
};

const installation_query = (engine_id: string) =>
	({
		...Trace,
		kind: "engine.installation.query",
		message_id: "message_engine.installation.query",
		payload: { engine_id },
	}) satisfies EngineInstallationQueryEnvelope;

const install_request = () =>
	({
		...Trace,
		kind: "engine.install.request",
		message_id: "message_engine.install.request",
		payload: { engine_id: "claude" },
	}) satisfies EngineInstallRequestEnvelope;

const authentication_request = () =>
	({
		...Trace,
		kind: "engine.authentication.request",
		message_id: "message_engine.authentication.request",
		payload: { engine_id: "claude" },
	}) satisfies EngineAuthenticationRequestEnvelope;

const rollback_request = () =>
	({
		...Trace,
		kind: "engine.rollback.request",
		message_id: "message_engine.rollback.request",
		payload: { engine_id: "claude" },
	}) satisfies EngineRollbackRequestEnvelope;

const handler = (toolchain: typeof EngineToolchain.Service) =>
	MakeEngineInstallationHandler.pipe(
		Effect.provide(Layer.mergeAll(MetadataLive, Layer.succeed(EngineToolchain, toolchain))),
	);

describe("engine installation handler", () => {
	it.effect("accepts an install only after the toolchain atomically enters installing", () =>
		Effect.gen(function* () {
			let installs = 0;
			const Handle = yield* handler(
				EngineToolchain.of({
					Install: () => Effect.die("Install must not be used by RPC"),
					List: () => Effect.succeed([]),
					ResolveSpawn: () => Effect.die("spawn resolution not used"),
					Rollback: () => Effect.die("rollback not used"),
					StartAuthentication: () => Effect.die("authentication not used"),
					StartInstall: () =>
						Effect.sync(() => {
							installs += 1;
							return status({
								activity: { _tag: "installing", phase: "resolving" },
								executable_path: "C:/artisan/toolchain/claude/2.1.220/claude.exe",
							});
						}),
					Status: () => Effect.die("Status must not race StartInstall"),
				}),
			);
			const result = (yield* Handle(
				install_request(),
			)) as EngineInstallationMutationResultEnvelope;

			expect(installs).toBe(1);
			expect(result.correlation_id).toBe("message_engine.install.request");
			expect(result.payload).toMatchObject({
				report: { activity: "installing", activity_phase: "resolving" },
				status: "accepted",
			});
			if (result.payload.status === "accepted") {
				expect(result.payload.report).not.toHaveProperty("executable_path");
				expect(result.payload.report).not.toHaveProperty("home_path");
			}
		}),
	);

	it.effect("rejects a second install deterministically when the toolchain is busy", () =>
		Effect.gen(function* () {
			const Handle = yield* handler(
				EngineToolchain.of({
					Install: () => Effect.die("Install must not be used by RPC"),
					List: () => Effect.succeed([]),
					ResolveSpawn: () => Effect.die("spawn resolution not used"),
					Rollback: () => Effect.die("rollback not used"),
					StartAuthentication: () => Effect.die("authentication not used"),
					StartInstall: (engine_id) =>
						Effect.fail(new EngineToolchainBusyError({ engine_id })),
					Status: () => Effect.die("status not used"),
				}),
			);
			const result = (yield* Handle(
				install_request(),
			)) as EngineInstallationMutationResultEnvelope;

			expect(result.payload).toEqual({
				message: "Another install is already running for this engine.",
				status: "rejected",
			});
		}),
	);

	it.effect(
		"returns no report for an unknown query filter and uses the recommended update target",
		() =>
			Effect.gen(function* () {
				const Handle = yield* handler(
					EngineToolchain.of({
						Install: () => Effect.die("install not used"),
						List: (options) => {
							expect(options).toEqual({ check_updates: true });
							return Effect.succeed([
								status({
									active_version: "2.1.230",
									latest_version: "2.2.0",
									recommended_version: "2.1.220",
								}),
							]);
						},
						ResolveSpawn: () => Effect.die("spawn resolution not used"),
						Rollback: () => Effect.die("rollback not used"),
						StartAuthentication: () => Effect.die("authentication not used"),
						StartInstall: () => Effect.die("install not used"),
						Status: () => Effect.die("status not used"),
					}),
				);
				const current = (yield* Handle({
					...installation_query("claude"),
					payload: { check_updates: true, engine_id: "claude" },
				})) as EngineInstallationQueryResultEnvelope;
				const unknown = (yield* Handle({
					...installation_query("unknown"),
					payload: { check_updates: true, engine_id: "unknown" },
				})) as EngineInstallationQueryResultEnvelope;

				expect(current.payload.engines[0]).toMatchObject({
					latest_version: "2.2.0",
					recommended_version: "2.1.220",
					update_available: false,
				});
				expect(unknown.payload.engines).toEqual([]);
			}),
	);

	it.effect("returns a completed rollback report as an accepted current result", () =>
		Effect.gen(function* () {
			const Handle = yield* handler(
				EngineToolchain.of({
					Install: () => Effect.die("install not used"),
					List: () => Effect.succeed([]),
					ResolveSpawn: () => Effect.die("spawn resolution not used"),
					Rollback: () =>
						Effect.succeed(
							status({ active_version: "2.1.220", previous_version: "2.1.230" }),
						),
					StartAuthentication: () => Effect.die("authentication not used"),
					StartInstall: () => Effect.die("install not used"),
					Status: () => Effect.die("status not used"),
				}),
			);
			const result = (yield* Handle(
				rollback_request(),
			)) as EngineInstallationMutationResultEnvelope;

			expect(result.payload).toMatchObject({
				report: {
					active_version: "2.1.220",
					previous_version: "2.1.230",
					activity: "idle",
				},
				status: "accepted",
			});
		}),
	);

	it.effect("rejects an unknown engine mutation with its request correlation", () =>
		Effect.gen(function* () {
			const Handle = yield* handler(
				EngineToolchain.of({
					Install: () => Effect.die("install not used"),
					List: () => Effect.succeed([]),
					ResolveSpawn: () => Effect.die("spawn resolution not used"),
					Rollback: () => Effect.die("rollback not used"),
					StartAuthentication: () => Effect.die("authentication not used"),
					StartInstall: (engine_id) =>
						Effect.fail(new EngineToolchainUnknownEngineError({ engine_id })),
					Status: () => Effect.die("status not used"),
				}),
			);
			const request = {
				...install_request(),
				message_id: "message_engine.install.unknown",
				payload: { engine_id: "unknown" },
			} satisfies EngineInstallRequestEnvelope;
			const result = (yield* Handle(request)) as EngineInstallationMutationResultEnvelope;

			expect(result.correlation_id).toBe("message_engine.install.unknown");
			expect(result.payload).toEqual({
				message: 'No managed distribution exists for engine "unknown".',
				status: "rejected",
			});
		}),
	);

	it.effect("returns rollback refusal as a normal correlated mutation result", () =>
		Effect.gen(function* () {
			const Handle = yield* handler(
				EngineToolchain.of({
					Install: () => Effect.die("install not used"),
					List: () => Effect.succeed([]),
					ResolveSpawn: () => Effect.die("spawn resolution not used"),
					Rollback: (engine_id) =>
						Effect.fail(new EngineToolchainRollbackUnavailableError({ engine_id })),
					StartAuthentication: () => Effect.die("authentication not used"),
					StartInstall: () => Effect.die("install not used"),
					Status: () => Effect.die("status not used"),
				}),
			);
			const result = (yield* Handle(
				rollback_request(),
			)) as EngineInstallationMutationResultEnvelope;

			expect(result.correlation_id).toBe("message_engine.rollback.request");
			expect(result.payload).toEqual({
				message: "No previous version is retained to roll back to.",
				status: "rejected",
			});
		}),
	);

	it.effect("accepts owned-home authentication after the toolchain begins it", () =>
		Effect.gen(function* () {
			const Handle = yield* handler(
				EngineToolchain.of({
					Install: () => Effect.die("install not used"),
					List: () => Effect.succeed([]),
					ResolveSpawn: () => Effect.die("spawn resolution not used"),
					Rollback: () => Effect.die("rollback not used"),
					StartAuthentication: () =>
						Effect.succeed(status({ activity: { _tag: "authenticating" } })),
					StartInstall: () => Effect.die("install not used"),
					Status: () => Effect.die("status not used"),
				}),
			);
			const result = (yield* Handle(
				authentication_request(),
			)) as EngineInstallationMutationResultEnvelope;

			expect(result.correlation_id).toBe("message_engine.authentication.request");
			expect(result.payload).toMatchObject({
				report: { activity: "authenticating" },
				status: "accepted",
			});
		}),
	);
});
