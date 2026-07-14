import { Context, Data, Effect, Schema } from "effect";

import { GitRemoteName } from "@artisan/protocol";

import type { GitTransportAuthorization } from "../git-provider/git-transport-authentication";
import { GitRemoteEndpoint } from "./git-mutation";

const ReferenceCount = Schema.Int.check(
	Schema.isGreaterThanOrEqualTo(0),
	Schema.isLessThanOrEqualTo(10_000),
);

/** Selects one exact configured remote for a metadata-only fetch. */
export const GitFetchRequest = Schema.Struct({
	remote: GitRemoteName,
	remote_endpoint: GitRemoteEndpoint,
});

export type GitFetchRequest = typeof GitFetchRequest.Type;

/** Summarizes one atomic remote-tracking reference refresh without exposing Git output. */
export const GitFetchResult = Schema.Struct({
	created_refs: ReferenceCount,
	deleted_refs: ReferenceCount,
	remote: GitRemoteName,
	remote_refs: ReferenceCount,
	updated_refs: ReferenceCount,
});

export type GitFetchResult = typeof GitFetchResult.Type;

/** Reports a metadata fetch failure without exposing credentials, endpoints, or process output. */
export class GitFetchError extends Data.TaggedError("GitFetchError")<{
	readonly cause?: unknown;
	readonly operation:
		| "configuration"
		| "fetch"
		| "integrity"
		| "invalid_authorization"
		| "invalid_request"
		| "precondition"
		| "process"
		| "settlement";
}> {}

/** Refreshes remote-tracking metadata without altering the visible checkout. */
export class GitFetch extends Context.Service<
	GitFetch,
	{
		readonly Fetch: (
			request: unknown,
			authorization: GitTransportAuthorization,
		) => Effect.Effect<GitFetchResult, GitFetchError>;
	}
>()("Artisan/GitFetch") {}
