import { Context, Data, Effect } from "effect";

/** Reports a Git checkout failure without exposing process implementation details. */
export class GitMutationError extends Data.TaggedError("GitMutationError")<{
	readonly cause: unknown;
	readonly operation: "checkout" | "configuration";
}> {}

/** Provides the deliberately small set of local Git mutations allowed by the adapter. */
export class GitMutation extends Context.Service<
	GitMutation,
	{
		readonly CheckoutLocalBranch: (branch: string) => Effect.Effect<void, GitMutationError>;
	}
>()("Artisan/GitMutation") {}
