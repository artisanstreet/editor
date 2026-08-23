import { Effect, Fiber } from "effect";
import { describe, expect, expectTypeOf, it } from "vitest";

import type {
	EngineAuthenticationRequest,
	EngineInstallRequest,
	EngineInstallationMutationResult,
	EngineInstallationMutationResultEnvelope,
	EngineInstallationQuery,
	EngineInstallationQueryResultEnvelope,
	EngineInstallationReport,
	EngineInstallationSnapshot,
	EngineRollbackRequest,
	InboundControlEnvelope,
} from "@artisan/protocol";
import type { ArtisanClient } from "@artisan/transport/client";
import type { ArtisanClientError } from "../../modules/transport/src/client-api/service";
import { RequestDelivered } from "../../modules/transport/src/internal/client-common";
import { ClientApiContext } from "../../modules/transport/src/internal/api/context";
import { MakeQueryApi } from "../../modules/transport/src/internal/api/queries";
import { make_client_request_coordinator } from "../../modules/transport/src/internal/client-request-coordinator";

/** Compile-time transport contract for managed provider installation operations. */
interface EngineInstallationClientContract extends Pick<
	typeof ArtisanClient.Service,
	"AuthenticateEngine" | "GetEngineInstallations" | "InstallEngine" | "RollbackEngine"
> {}

const public_methods: ReadonlyArray<keyof EngineInstallationClientContract> = [
	"AuthenticateEngine",
	"GetEngineInstallations",
	"InstallEngine",
	"RollbackEngine",
];

const report: EngineInstallationReport = {
	activity: "idle" as const,
	credentials_present: true,
	display_name: "Claude",
	engine_id: "claude",
	managed: true,
};

type EngineInstallationRequest = Extract<
	InboundControlEnvelope,
	{
		kind:
			| "engine.installation.query"
			| "engine.install.request"
			| "engine.authentication.request"
			| "engine.rollback.request";
	}
>;

const backend_trace = (correlation_id: string) => ({
	correlation_id,
	message_id: `result_${correlation_id}`,
	origin: "backend" as const,
	protocol_version: 1 as const,
	schema_version: 1 as const,
	sent_at: "2026-08-14T12:00:00.000Z",
});

const installation_query_result = (
	correlation_id: string,
): EngineInstallationQueryResultEnvelope => ({
	...backend_trace(correlation_id),
	kind: "engine.installation.query.result",
	payload: {
		engines: [report],
		fetched_at: "2026-08-14T12:00:00.000Z",
	},
});

const installation_mutation_result = (
	correlation_id: string,
): EngineInstallationMutationResultEnvelope => ({
	...backend_trace(correlation_id),
	kind: "engine.installation.mutation.result",
	payload: { report, status: "accepted" },
});

const response_for = (request: EngineInstallationRequest) => {
	switch (request.kind) {
		case "engine.installation.query":
			return installation_query_result(request.message_id);
		case "engine.install.request":
		case "engine.authentication.request":
		case "engine.rollback.request":
			return installation_mutation_result(request.message_id);
	}
};

describe("engine installation client API", () => {
	it("exposes polling and normal-result mutations through the renderer-safe client", () => {
		expect(public_methods).toEqual([
			"AuthenticateEngine",
			"GetEngineInstallations",
			"InstallEngine",
			"RollbackEngine",
		]);
		expectTypeOf<EngineInstallationClientContract["GetEngineInstallations"]>().toEqualTypeOf<
			(
				input?: EngineInstallationQuery,
			) => Effect.Effect<EngineInstallationSnapshot, ArtisanClientError>
		>();
		expectTypeOf<EngineInstallationClientContract["InstallEngine"]>().toEqualTypeOf<
			(
				input: EngineInstallRequest,
			) => Effect.Effect<EngineInstallationMutationResult, ArtisanClientError>
		>();
		expectTypeOf<EngineInstallationClientContract["AuthenticateEngine"]>().toEqualTypeOf<
			(
				input: EngineAuthenticationRequest,
			) => Effect.Effect<EngineInstallationMutationResult, ArtisanClientError>
		>();
		expectTypeOf<EngineInstallationClientContract["RollbackEngine"]>().toEqualTypeOf<
			(
				input: EngineRollbackRequest,
			) => Effect.Effect<EngineInstallationMutationResult, ArtisanClientError>
		>();
	});

	it("sends each managed-provider operation and resolves its correlated result", async () => {
		const program = Effect.gen(function* () {
			const sent: Array<EngineInstallationRequest> = [];
			const requests = yield* make_client_request_coordinator((envelope) =>
				Effect.gen(function* () {
					switch (envelope.kind) {
						case "engine.installation.query":
						case "engine.install.request":
						case "engine.authentication.request":
						case "engine.rollback.request":
							sent.push(envelope);
							return RequestDelivered("connection_installation");
						default:
							return yield* Effect.die(
								`unexpected engine-installation request ${envelope.kind}`,
							);
					}
				}),
			);
			let trace_number = 0;
			const api = yield* MakeQueryApi.pipe(
				Effect.provideService(ClientApiContext, {
					MakeId: (prefix) => Effect.succeed(`${prefix}_test`),
					MakeTrace: Effect.sync(() => ({
						message_id: `message_${++trace_number}`,
						origin: "frontend" as const,
						protocol_version: 1 as const,
						schema_version: 1 as const,
						sent_at: "2026-08-14T12:00:00.000Z",
					})),
					Request: requests.Request,
				}),
			);
			const pending = yield* Effect.all(
				{
					installations: Effect.forkScoped(
						api.get_engine_installations({ check_updates: true, engine_id: "claude" }),
					),
					install: Effect.forkScoped(
						api.install_engine({ engine_id: "claude", version: "2.1.220" }),
					),
					authentication: Effect.forkScoped(
						api.authenticate_engine({ engine_id: "claude" }),
					),
					rollback: Effect.forkScoped(api.rollback_engine({ engine_id: "claude" })),
				},
				{ concurrency: "unbounded" },
			);

			yield* Effect.yieldNow;
			expect(sent.map(({ kind, payload }) => ({ kind, payload }))).toEqual([
				{
					kind: "engine.installation.query",
					payload: { check_updates: true, engine_id: "claude" },
				},
				{
					kind: "engine.install.request",
					payload: { engine_id: "claude", version: "2.1.220" },
				},
				{ kind: "engine.authentication.request", payload: { engine_id: "claude" } },
				{ kind: "engine.rollback.request", payload: { engine_id: "claude" } },
			]);
			yield* Effect.forEach(sent, (request) => requests.Resolve(response_for(request)), {
				discard: true,
			});

			return yield* Effect.all({
				authentication: Fiber.join(pending.authentication),
				install: Fiber.join(pending.install),
				installations: Fiber.join(pending.installations),
				rollback: Fiber.join(pending.rollback),
			});
		}).pipe(Effect.scoped);

		const { installations, install, authentication, rollback } =
			await Effect.runPromise(program);
		expect(installations).toEqual({
			engines: [report],
			fetched_at: "2026-08-14T12:00:00.000Z",
		});
		expect(install).toEqual({ report, status: "accepted" });
		expect(authentication).toEqual({ report, status: "accepted" });
		expect(rollback).toEqual({ report, status: "accepted" });
	});
});
