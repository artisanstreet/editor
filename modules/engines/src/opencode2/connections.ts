import { Effect } from "effect";

import {
	EngineProtocolError,
	type EngineCatalogScope,
	type EngineConnectionInfo,
	type EngineConnectionManager,
	type EngineConnectionMethod,
	type EngineFailure,
} from "../engine";
import { OpenCode2ApiError } from "./protocol";
import type { OpenCode2PrivateService } from "./service";

export type OpenCode2ServiceFor = (
	scope: EngineCatalogScope,
) => Effect.Effect<OpenCode2PrivateService, EngineFailure>;

const map_api = <A, R>(effect: Effect.Effect<A, OpenCode2ApiError, R>) =>
	effect.pipe(
		Effect.mapError(
			(failure) =>
				new EngineProtocolError({
					engine_id: "opencode2",
					message:
						failure.code === "http"
							? `OpenCode rejected ${failure.operation} with HTTP ${String(failure.status ?? "error")}.`
							: `OpenCode ${failure.operation} failed its ${failure.code} boundary.`,
				}),
		),
	);

/**
 * The certified OpenCode build joins the Console base path to the device
 * endpoint even when that endpoint already begins with `/console`. The
 * resulting `/console/console/device` URL redirects to the account overview
 * instead of authorizing the pending CLI device. Repair that one upstream
 * shape at our trust boundary before exposing the URL to the renderer.
 */
export const normalize_opencode2_authorization_url = (value: string) => {
	const url = new URL(value);
	const local_http =
		url.protocol === "http:" && new Set(["127.0.0.1", "[::1]"]).has(url.hostname);
	if (
		(url.protocol !== "https:" && !local_http) ||
		url.username.length > 0 ||
		url.password.length > 0
	)
		throw new Error("Unsafe authorization URL");
	if (
		url.protocol === "https:" &&
		url.hostname === "opencode.ai" &&
		url.pathname.startsWith("/console/console/")
	)
		url.pathname = url.pathname.replace(/^\/console\/console(?=\/)/, "/console");
	return url.toString();
};

const authorization_url = (value: string) =>
	Effect.try({
		try: () => normalize_opencode2_authorization_url(value),
		catch: () =>
			new EngineProtocolError({
				engine_id: "opencode2",
				message: "OpenCode returned an unsafe authorization URL.",
			}),
	});

const connection_method = (value: unknown): EngineConnectionMethod | undefined => {
	if (typeof value !== "object" || value === null || Array.isArray(value)) return undefined;
	const method = value as Readonly<Record<string, unknown>>;
	const type = typeof method.type === "string" ? method.type : undefined;
	const label = typeof method.label === "string" ? method.label : type;
	if (label === undefined) return undefined;
	if (type === "key") return { label, type };
	if (type === "env")
		return {
			label,
			names: Array.isArray(method.names)
				? method.names.filter((name): name is string => typeof name === "string")
				: [],
			type,
		};
	if ((type === "oauth" || type === "command") && typeof method.id === "string")
		return { id: method.id, label, type };
	return undefined;
};

export const MakeOpenCode2Connections = (
	ServiceFor: OpenCode2ServiceFor,
): EngineConnectionManager => ({
	BeginOAuth: (scope, integration_id, method_id) =>
		Effect.gen(function* () {
			const service = yield* ServiceFor(scope);
			const attempt = yield* map_api(
				service.Client.BeginOAuth(scope, integration_id, method_id),
			);
			return {
				attempt_id: attempt.attemptID,
				expires_at_ms: attempt.time.expires,
				instructions: attempt.instructions,
				mode: attempt.mode,
				url: yield* authorization_url(attempt.url),
			};
		}),
	CancelOAuth: (scope, integration_id, attempt_id) =>
		Effect.gen(function* () {
			const service = yield* ServiceFor(scope);
			yield* map_api(service.Client.CancelOAuth(scope, integration_id, attempt_id));
		}),
	CompleteOAuth: (scope, integration_id, attempt_id, code) =>
		Effect.gen(function* () {
			const service = yield* ServiceFor(scope);
			yield* map_api(service.Client.CompleteOAuth(scope, integration_id, attempt_id, code));
		}),
	ConnectKey: (scope, integration_id, key) =>
		Effect.gen(function* () {
			const service = yield* ServiceFor(scope);
			yield* map_api(service.Client.ConnectKey(scope, integration_id, key));
		}),
	List: (scope) =>
		Effect.gen(function* () {
			const service = yield* ServiceFor(scope);
			const integrations = yield* map_api(service.Client.Integrations(scope));
			return integrations.map(
				(integration): EngineConnectionInfo => ({
					connected: integration.connections.length > 0,
					id: integration.id,
					methods: integration.methods.flatMap((method) => {
						const normalized = connection_method(method);
						return normalized === undefined ? [] : [normalized];
					}),
					name: integration.name,
				}),
			);
		}),
	OAuthStatus: (scope, integration_id, attempt_id) =>
		Effect.gen(function* () {
			const service = yield* ServiceFor(scope);
			return yield* map_api(service.Client.OAuthStatus(scope, integration_id, attempt_id));
		}),
});
