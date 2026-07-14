import { Context, Data, Effect } from "effect";

/** Identifies the exact provider account permitted to authenticate one child Git invocation. */
export interface GitTransportAuthenticationRequest {
	readonly account_login: string;
	readonly host: string;
	readonly provider_id: string;
	readonly remote_endpoint: string;
}

/** Carries backend-private environment overrides for one scoped child Git invocation. */
export interface GitTransportAuthorization {
	readonly environment: Readonly<Record<string, string | undefined>>;
	readonly git_executable_path: string;
	readonly remote_endpoint: string;
	readonly transport_protocol: "file" | "https" | "ssh";
}

/** Reports an authorization setup failure without retaining provider credentials or process output. */
export class GitTransportAuthenticationError extends Data.TaggedError(
	"GitTransportAuthenticationError",
)<{
	readonly reason:
		| "authentication_required"
		| "dependency_missing"
		| "invalid_request"
		| "unsupported_provider";
}> {}

/** Authorizes a bounded child Git invocation without exposing provider credentials to Artisan. */
export class GitTransportAuthentication extends Context.Service<
	GitTransportAuthentication,
	{
		readonly WithAuthorization: <A, E, R>(
			input: GitTransportAuthenticationRequest,
			use: (authorization: GitTransportAuthorization) => Effect.Effect<A, E, R>,
		) => Effect.Effect<
			A,
			E | GitTransportAuthenticationError,
			Exclude<R, import("effect").Scope.Scope>
		>;
	}
>()("Artisan/GitTransportAuthentication") {}
