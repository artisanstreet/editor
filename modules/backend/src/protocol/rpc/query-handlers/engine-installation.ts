import { Effect } from "effect";

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
	EngineToolchain,
	type EngineToolchainStatus,
} from "@artisan/engines";

import { RuntimeMetadata } from "../../../runtime/metadata";

const max_engines_per_snapshot = 16;

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
		credentials_present: status.credentials_present,
		display_name: status.display_name,
		engine_id: status.engine_id,
		...(status.activity._tag === "failed" ? { failure: status.activity.message } : {}),
		...(status.latest_version === undefined ? {} : { latest_version: status.latest_version }),
		managed: status.active_version !== undefined,
		...(status.minimum_version === undefined
			? {}
			: { minimum_version: status.minimum_version }),
		...(status.previous_version === undefined
			? {}
			: { previous_version: status.previous_version }),
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
			const read_options =
				query.payload.check_updates === true ? { check_updates: true } : {};
			const statuses = (yield* toolchain.List(read_options)).filter(
				(status) =>
					query.payload.engine_id === undefined ||
					status.engine_id === query.payload.engine_id,
			);
			const fetched_at = yield* metadata.Now;

			return yield* Envelope(query, "engine.installation.query.result" as const, {
				engines: statuses.slice(0, max_engines_per_snapshot).map(to_installation_report),
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
		toolchain.StartAuthentication(request.payload.engine_id).pipe(
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
				return HandleInstall(request);
			case "engine.authentication.request":
				return HandleAuthentication(request);
			case "engine.rollback.request":
				return HandleRollback(request);
		}
	};
});
