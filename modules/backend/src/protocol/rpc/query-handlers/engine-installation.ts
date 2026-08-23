import { Effect, Option, Ref } from "effect";

import type {
  EngineInstallationMutationResultEnvelope,
  EngineInstallationQueryEnvelope,
  EngineInstallationQueryResultEnvelope,
  EngineInstallationReport,
  EngineInstallRequestEnvelope,
  EngineAuthenticationRequestEnvelope,
  EngineRollbackRequestEnvelope,
  ProtocolErrorDetail,
} from "@artisan/protocol";
import {
  compare_toolchain_versions,
  describe_engine_toolchain_failure,
  EngineRegistry,
  EngineToolchain,
  type EngineCatalogScope,
  type EngineOAuthAttempt,
  type EngineToolchainStatus,
} from "@artisan/engines";

import { RuntimeMetadata } from "../../../runtime/metadata";
import { ProductTelemetry } from "../../../telemetry/product-telemetry";

const max_engines_per_snapshot = 16;

interface PendingEngineAuthorization {
  readonly action: EngineOAuthAttempt;
  readonly integration_id: string;
  readonly scope: EngineCatalogScope;
  readonly status_failures: number;
}

const maximum_authorization_status_failures = 8;

function to_installation_report(status: EngineToolchainStatus): EngineInstallationReport {
  const update_target = status.recommended_version ?? status.latest_version;
  const update_available =
    status.active_version !== undefined &&
    update_target !== undefined &&
    compare_toolchain_versions(update_target, status.active_version) > 0;
  return {
    ...(status.active_version === undefined ? {} : { active_version: status.active_version }),
    activity: status.activity._tag,
    ...(status.activity._tag === "installing" ? { activity_phase: status.activity.phase } : {}),
		...(status.activity._tag === "installing" && status.activity.detail !== undefined
			? { activity_detail: status.activity.detail }
			: {}),
    credentials_present: status.credentials_present,
    display_name: status.display_name,
    engine_id: status.engine_id,
    ...(status.activity._tag === "failed" ? { failure: status.activity.message } : {}),
    ...(status.latest_version === undefined ? {} : { latest_version: status.latest_version }),
    managed: status.active_version !== undefined,
    ...(status.minimum_version === undefined ? {} : { minimum_version: status.minimum_version }),
    ...(status.previous_version === undefined ? {} : { previous_version: status.previous_version }),
    ...(status.recommended_version === undefined
      ? {}
      : { recommended_version: status.recommended_version }),
    ...(update_target === undefined ? {} : { update_available }),
  };
}

type EngineInstallationEnvelope =
  | EngineInstallationQueryEnvelope
  | EngineInstallRequestEnvelope
  | EngineAuthenticationRequestEnvelope
  | EngineRollbackRequestEnvelope;

export const MakeEngineInstallationHandler = Effect.gen(function* () {
  const toolchain = yield* EngineToolchain;
  const metadata = yield* RuntimeMetadata;
  const product_telemetry = Option.getOrElse(yield* Effect.serviceOption(ProductTelemetry), () =>
    ProductTelemetry.of({ Capture: () => Effect.void }),
  );
  const registry = Option.getOrUndefined(yield* Effect.serviceOption(EngineRegistry));
  const authorizations = yield* Ref.make<ReadonlyMap<string, PendingEngineAuthorization>>(
    new Map(),
  );

  const ResolveConnectionReport = (status: EngineToolchainStatus) =>
    Effect.gen(function* () {
      const base = to_installation_report(status);
      if (registry === undefined || status.active_version === undefined) return base;
      const engine_result = yield* registry.Get(status.engine_id).pipe(Effect.result);
      if (engine_result._tag === "Failure" || engine_result.success.Connections === undefined)
        return base;
      const connections = engine_result.success.Connections;
      const pending = (yield* Ref.get(authorizations)).get(status.engine_id);
      if (pending !== undefined) {
        const attempt_status = yield* connections
          .OAuthStatus(pending.scope, pending.integration_id, pending.action.attempt_id)
          .pipe(Effect.result);
        if (attempt_status._tag === "Failure") {
          /**
           * A completed device flow may persist its connection immediately
           * before the attempt-status response becomes unavailable. Prefer
           * that durable connection state over the ephemeral attempt handle.
           */
          const listed = yield* connections.List(pending.scope).pipe(Effect.result);
          if (
            listed._tag === "Success" &&
            listed.success.some(
              (item) => item.id === pending.integration_id && item.connected,
            )
          ) {
            yield* Ref.update(authorizations, (current) => {
              const next = new Map(current);
              next.delete(status.engine_id);
              return next;
            });
            return { ...base, credentials_present: true };
          }
          const status_failures = pending.status_failures + 1;
          if (status_failures >= maximum_authorization_status_failures) {
            yield* Ref.update(authorizations, (current) => {
              const next = new Map(current);
              next.delete(status.engine_id);
              return next;
            });
            return {
              ...base,
              activity: "failed" as const,
              failure: "The OpenCode sign-in session was interrupted. Try again.",
            };
          }
          yield* Ref.update(authorizations, (current) =>
            new Map(current).set(status.engine_id, { ...pending, status_failures }),
          );
          return {
            ...base,
            activity: "authenticating" as const,
            authorization: pending.action,
          };
        }
        if (attempt_status.success.status === "pending")
          return {
            ...base,
            activity: "authenticating" as const,
            authorization: pending.action,
          };
        yield* Ref.update(authorizations, (current) => {
          const next = new Map(current);
          next.delete(status.engine_id);
          return next;
        });
        if (attempt_status.success.status === "complete")
          return { ...base, credentials_present: true };
        return {
          ...base,
          activity: "failed" as const,
          failure:
            attempt_status.success.status === "failed"
              ? attempt_status.success.message
              : "The OpenCode Console authorization attempt expired.",
        };
      }
      const listed = yield* connections
        .List({
          profile_id: "default",
          working_directory: process.cwd(),
          workspace_trust: "safe",
        })
        .pipe(Effect.result);
      return listed._tag === "Success"
        ? { ...base, credentials_present: listed.success.some((item) => item.connected) }
        : base;
    });

  const Envelope = <Kind extends string, Payload>(
    request: EngineInstallationEnvelope,
    kind: Kind,
    payload: Payload,
  ) =>
    Effect.gen(function* () {
      const message_id = yield* metadata.MakeId("message");
      const sent_at = yield* metadata.Now;

      return {
        correlation_id: request.message_id,
        kind,
        message_id,
        origin: "backend" as const,
        payload,
        protocol_version: 1 as const,
        schema_version: 1 as const,
        sent_at,
      };
    });

  const HandleQuery = (
    query: EngineInstallationQueryEnvelope,
  ): Effect.Effect<EngineInstallationQueryResultEnvelope, ProtocolErrorDetail> =>
    Effect.gen(function* () {
      const read_options = query.payload.check_updates === true ? { check_updates: true } : {};
      const statuses = (yield* toolchain.List(read_options)).filter(
        (status) =>
          query.payload.engine_id === undefined || status.engine_id === query.payload.engine_id,
      );
      const fetched_at = yield* metadata.Now;

      return yield* Envelope(query, "engine.installation.query.result" as const, {
        engines: yield* Effect.forEach(
          statuses.slice(0, max_engines_per_snapshot),
          ResolveConnectionReport,
        ),
        fetched_at,
      });
    });

  const MutationResult = (
    request:
      | EngineInstallRequestEnvelope
      | EngineAuthenticationRequestEnvelope
      | EngineRollbackRequestEnvelope,
    payload: EngineInstallationMutationResultEnvelope["payload"],
  ) => Envelope(request, "engine.installation.mutation.result" as const, payload);

  /**
   * The toolchain confirms acceptance before returning its current report. Its
   * layer owns background install/auth work, while rollback may have completed
   * synchronously. Callers poll service-scoped installation state; artifacts
   * are outside the database transaction and this milestone intentionally has
   * no journal or subscription stream.
   */
  const HandleInstall = (
    request: EngineInstallRequestEnvelope,
  ): Effect.Effect<EngineInstallationMutationResultEnvelope, ProtocolErrorDetail> =>
    Effect.gen(function* () {
      const status = yield* toolchain
        .StartInstall(request.payload.engine_id, request.payload.version)
        .pipe(
          Effect.matchEffect({
            onFailure: (failure) =>
              MutationResult(request, {
                message: describe_engine_toolchain_failure(failure),
                status: "rejected" as const,
              }),
            onSuccess: (current) =>
              MutationResult(request, {
                report: to_installation_report(current),
                status: "accepted" as const,
              }),
          }),
        );
      return status;
    });

  const HandleRollback = (
    request: EngineRollbackRequestEnvelope,
  ): Effect.Effect<EngineInstallationMutationResultEnvelope, ProtocolErrorDetail> =>
    toolchain.Rollback(request.payload.engine_id).pipe(
      Effect.matchEffect({
        onFailure: (failure) =>
          MutationResult(request, {
            message: describe_engine_toolchain_failure(failure),
            status: "rejected" as const,
          }),
        onSuccess: (status) =>
          MutationResult(request, {
            report: to_installation_report(status),
            status: "accepted" as const,
          }),
      }),
    );

  const HandleAuthentication = (
    request: EngineAuthenticationRequestEnvelope,
  ): Effect.Effect<EngineInstallationMutationResultEnvelope, ProtocolErrorDetail> =>
    Effect.gen(function* () {
      if (registry !== undefined) {
        const engine_result = yield* registry.Get(request.payload.engine_id).pipe(Effect.result);
        if (engine_result._tag === "Success" && engine_result.success.Connections !== undefined) {
          const status_result = yield* toolchain
            .Status(request.payload.engine_id)
            .pipe(Effect.result);
          if (status_result._tag === "Failure")
            return yield* MutationResult(request, {
              message: describe_engine_toolchain_failure(status_result.failure),
              status: "rejected" as const,
            });
          const status = status_result.success;
          if (status.active_version === undefined)
            return yield* MutationResult(request, {
              message: "Install the managed OpenCode binary before signing in.",
              status: "rejected" as const,
            });
          const scope: EngineCatalogScope = {
            profile_id: request.payload.profile_id ?? "default",
            working_directory: request.payload.working_directory ?? process.cwd(),
            workspace_trust: "safe",
          };
          const connections = yield* engine_result.success.Connections.List(scope).pipe(
            Effect.result,
          );
          if (connections._tag === "Failure")
            return yield* MutationResult(request, {
              message: "OpenCode connection methods could not be read.",
              status: "rejected" as const,
            });
          const integration =
            connections.success.find((item) => item.id === "opencode") ?? connections.success[0];
          if (integration?.connected === true)
            return yield* MutationResult(request, {
              report: {
                ...to_installation_report(status),
                credentials_present: true,
              },
              status: "accepted" as const,
            });
          const method = integration?.methods.find((candidate) => candidate.type === "oauth");
          if (integration === undefined || method?.type !== "oauth")
            return yield* MutationResult(request, {
              message: "OpenCode Console did not expose an OAuth sign-in method.",
              status: "rejected" as const,
            });
          const attempt = yield* engine_result.success.Connections.BeginOAuth(
            scope,
            integration.id,
            method.id,
          ).pipe(Effect.result);
          if (attempt._tag === "Failure")
            return yield* MutationResult(request, {
              message: "OpenCode Console sign-in could not be started.",
              status: "rejected" as const,
            });
          yield* Ref.update(authorizations, (current) =>
            new Map(current).set(request.payload.engine_id, {
              action: attempt.success,
              integration_id: integration.id,
              scope,
              status_failures: 0,
            }),
          );
          return yield* MutationResult(request, {
            report: {
              ...to_installation_report(status),
              activity: "authenticating",
              authorization: attempt.success,
            },
            status: "accepted" as const,
          });
        }
      }
      return yield* toolchain.StartAuthentication(request.payload.engine_id).pipe(
        Effect.matchEffect({
          onFailure: (failure) =>
            MutationResult(request, {
              message: describe_engine_toolchain_failure(failure),
              status: "rejected" as const,
            }),
          onSuccess: (current) =>
            MutationResult(request, {
              report: to_installation_report(current),
              status: "accepted" as const,
            }),
        }),
      );
    });

  return (
    request: EngineInstallationEnvelope,
  ): Effect.Effect<
    EngineInstallationQueryResultEnvelope | EngineInstallationMutationResultEnvelope,
    ProtocolErrorDetail
  > => {
    switch (request.kind) {
      case "engine.installation.query":
        return HandleQuery(request);
      case "engine.install.request":
        return (() => {
          const started_at = Date.now();
          return HandleInstall(request).pipe(
            Effect.tap((result) =>
              product_telemetry.Capture(
                {
                  event: "engine_setup_finished",
                  properties: {
                    duration_ms: Math.min(604_800_000, Date.now() - started_at),
                    engine_id: request.payload.engine_id,
                    ...(result.payload.status === "rejected"
                      ? { failure_code: "unknown" as const }
                      : {}),
                    operation: "install",
                    outcome: result.payload.status === "accepted" ? "succeeded" : "failed",
                  },
                },
                `engine_install:${request.message_id}`,
              ),
            ),
          );
        })();
      case "engine.authentication.request":
        return (() => {
          const started_at = Date.now();
          return HandleAuthentication(request).pipe(
            Effect.tap((result) =>
              product_telemetry.Capture(
                {
                  event: "engine_setup_finished",
                  properties: {
                    duration_ms: Math.min(604_800_000, Date.now() - started_at),
                    engine_id: request.payload.engine_id,
                    ...(result.payload.status === "rejected"
                      ? { failure_code: "authentication" as const }
                      : {}),
                    operation: "authenticate",
                    outcome: result.payload.status === "accepted" ? "succeeded" : "failed",
                  },
                },
                `engine_authentication:${request.message_id}`,
              ),
            ),
          );
        })();
      case "engine.rollback.request":
        return HandleRollback(request);
    }
  };
});
