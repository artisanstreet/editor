import { createHash } from "node:crypto";

import { Context, Effect, Layer } from "effect";
import type { CapabilityDetail } from "@artisan/protocol";

import { CapabilityRepository } from "./repository";
import { CapabilityServiceError } from "./contracts";
import {
	OAuth,
	type OAuthBeginInput,
	type OAuthCompletionInput,
	type OAuthTokenStatus,
} from "./oauth";

const Fingerprint = (value: unknown) =>
	createHash("sha256").update(JSON.stringify(value)).digest("hex");

export class CapabilityOAuthLifecycle extends Context.Service<
	CapabilityOAuthLifecycle,
	{
		readonly Begin: (
			input: OAuthBeginInput & { readonly operation_id: string },
		) => Effect.Effect<
			{
				readonly _tag: "started";
				readonly authorization_url: string;
				readonly state: string;
			},
			unknown
		>;
		readonly Complete: (
			input: OAuthCompletionInput & { readonly operation_id: string },
		) => Effect.Effect<OAuthTokenStatus, unknown>;
		readonly Refresh: (input: {
			readonly capability_id: string;
			readonly operation_id: string;
		}) => Effect.Effect<OAuthTokenStatus, unknown>;
		readonly Revoke: (input: {
			readonly capability_id: string;
			readonly operation_id: string;
		}) => Effect.Effect<void, unknown>;
		readonly Status: (capability_id: string) => Effect.Effect<OAuthTokenStatus, unknown>;
	}
>()("Artisan/Marketplace/CapabilityOAuthLifecycle") {}

export const CapabilityOAuthLifecycleLive = Layer.effect(
	CapabilityOAuthLifecycle,
	Effect.gen(function* () {
		const repository = yield* CapabilityRepository;
		const oauth = yield* OAuth;
		const OAuthDetail = (capability_id: string) =>
			Effect.gen(function* () {
				const detail = yield* repository.ReadDetail(capability_id);
				if (detail.auth.kind !== "oauth")
					return yield* new CapabilityServiceError({
						code: "policy_denied",
						message: "Capability does not use OAuth",
					});
				return detail;
			});
		const TokenStatusFromDetail = (detail: CapabilityDetail): OAuthTokenStatus => {
			if (detail.auth.kind !== "oauth") return { capability_id: detail.id, state: "absent" };
			return {
				capability_id: detail.id,
				...(detail.auth.token_ref === undefined
					? {}
					: { secret_reference: detail.auth.token_ref }),
				state:
					detail.auth.token_status === "authorized"
						? "active"
						: detail.auth.token_status === "not_started"
							? "absent"
							: "expired",
			};
		};
		const Begin = (input: OAuthBeginInput & { readonly operation_id: string }) =>
			Effect.gen(function* () {
				const detail = yield* OAuthDetail(input.capability_id);
				yield* repository.RecordOAuthOperation({
					capability_id: input.capability_id,
					kind: "oauth_begin",
					operation_id: input.operation_id,
					request_fingerprint: Fingerprint({
						authorization_url: input.authorization_url,
						scopes: input.scopes,
					}),
				});
				const claim = yield* repository.ClaimOAuthOperation(input.operation_id);
				if (claim === "completed")
					return {
						_tag: "started" as const,
						...(yield* repository.ReadOAuthBeginResult(input.operation_id)),
					};
				if (claim === "executing")
					return yield* new CapabilityServiceError({
						code: "policy_denied",
						message:
							"OAuth begin outcome is ambiguous and requires explicit reconciliation",
					});
				const result = yield* oauth.Begin({
					authorization_url: input.authorization_url,
					capability_id: input.capability_id,
					scopes: input.scopes,
				});
				yield* repository.CompleteOAuthOperation({
					begin_result: result,
					operation: "oauth_started",
					operation_id: input.operation_id,
					status: detail.status,
				});
				return { _tag: "started" as const, ...result };
			});
		const Complete = (input: OAuthCompletionInput & { readonly operation_id: string }) =>
			Effect.gen(function* () {
				const detail = yield* OAuthDetail(input.capability_id);
				yield* repository.RecordOAuthOperation({
					capability_id: input.capability_id,
					kind: "oauth_complete",
					operation_id: input.operation_id,
					request_fingerprint: Fingerprint(input.callback_reference),
				});
				const claim = yield* repository.ClaimOAuthOperation(input.operation_id);
				if (claim === "completed")
					return TokenStatusFromDetail(yield* OAuthDetail(input.capability_id));
				if (claim === "executing")
					return yield* new CapabilityServiceError({
						code: "policy_denied",
						message:
							"OAuth completion outcome is ambiguous and requires explicit reconciliation",
					});
				const result = yield* oauth.Complete({
					capability_id: input.capability_id,
					callback_reference: input.callback_reference,
				});
				yield* repository.CompleteOAuthOperation({
					operation: "oauth_completed",
					operation_id: input.operation_id,
					status: detail.status,
					token_status: result,
				});
				return result;
			});
		const Refresh = (input: {
			readonly capability_id: string;
			readonly operation_id: string;
		}) =>
			Effect.gen(function* () {
				const detail = yield* OAuthDetail(input.capability_id);
				yield* repository.RecordOAuthOperation({
					capability_id: input.capability_id,
					kind: "oauth_refresh",
					operation_id: input.operation_id,
					request_fingerprint: Fingerprint({ capability_id: input.capability_id }),
				});
				const claim = yield* repository.ClaimOAuthOperation(input.operation_id);
				if (claim === "completed")
					return TokenStatusFromDetail(yield* OAuthDetail(input.capability_id));
				if (claim === "executing")
					return yield* new CapabilityServiceError({
						code: "policy_denied",
						message:
							"OAuth refresh outcome is ambiguous and requires explicit reconciliation",
					});
				const result = yield* oauth.Refresh(input.capability_id);
				yield* repository.CompleteOAuthOperation({
					operation: "oauth_refreshed",
					operation_id: input.operation_id,
					status: detail.status,
					token_status: result,
				});
				return result;
			});
		const Revoke = (input: { readonly capability_id: string; readonly operation_id: string }) =>
			Effect.gen(function* () {
				const detail = yield* OAuthDetail(input.capability_id);
				yield* repository.RecordOAuthOperation({
					capability_id: input.capability_id,
					kind: "oauth_revoke",
					operation_id: input.operation_id,
					request_fingerprint: Fingerprint({ capability_id: input.capability_id }),
				});
				const claim = yield* repository.ClaimOAuthOperation(input.operation_id);
				if (claim === "completed") return;
				if (claim === "executing")
					return yield* new CapabilityServiceError({
						code: "policy_denied",
						message:
							"OAuth revoke outcome is ambiguous and requires explicit reconciliation",
					});
				yield* oauth.Revoke(input.capability_id);
				yield* repository.CompleteOAuthOperation({
					operation: "oauth_revoked",
					operation_id: input.operation_id,
					status: detail.status,
					token_status: { capability_id: input.capability_id, state: "revoked" },
				});
			});
		const Status = (capability_id: string) =>
			OAuthDetail(capability_id).pipe(Effect.map(TokenStatusFromDetail));
		return { Begin, Complete, Refresh, Revoke, Status };
	}),
);
