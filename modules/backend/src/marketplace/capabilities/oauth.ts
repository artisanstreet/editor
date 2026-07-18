import { Context, Data, Effect, Layer } from "effect";
import type { SecretReference } from "@artisan/protocol";

export interface OAuthBeginInput {
	readonly authorization_url: string;
	readonly capability_id: string;
	readonly scopes: ReadonlyArray<string>;
}
export interface OAuthCompletionInput {
	readonly capability_id: string;
	/** Opaque resolver reference; the adapter validates provider code/state internally. */
	readonly callback_reference: string;
}
export interface OAuthTokenStatus {
	readonly capability_id: string;
	readonly state: "absent" | "active" | "expired" | "revoked";
	readonly secret_reference?: SecretReference;
}

/** Safe OAuth boundary error; provider tokens and authorization codes never escape it. */
export class OAuthError extends Data.TaggedError("OAuthError")<{
	readonly operation: "begin" | "complete" | "refresh" | "revoke" | "status" | "unavailable";
}> {}

/** Injectable OAuth adapter. Calling Begin is the only operation allowed to begin a browser flow. */
export class OAuthAdapter extends Context.Service<
	OAuthAdapter,
	{
		readonly Begin: (
			input: OAuthBeginInput,
		) => Effect.Effect<
			{ readonly authorization_url: string; readonly state: string },
			OAuthError
		>;
		readonly Complete: (
			input: OAuthCompletionInput,
		) => Effect.Effect<OAuthTokenStatus, OAuthError>;
		readonly Refresh: (capability_id: string) => Effect.Effect<OAuthTokenStatus, OAuthError>;
		readonly Revoke: (capability_id: string) => Effect.Effect<void, OAuthError>;
		readonly Status: (capability_id: string) => Effect.Effect<OAuthTokenStatus, OAuthError>;
	}
>()("Artisan/Marketplace/OAuthAdapter") {}

/** Public OAuth service; Layer acquisition never begins or refreshes a flow. */
export class OAuth extends Context.Service<OAuth, OAuthAdapter["Service"]>()(
	"Artisan/Marketplace/OAuth",
) {}

export const make_oauth_layer = Layer.effect(OAuth, Effect.service(OAuthAdapter));
export const EmptyOAuthAdapterLive = Layer.succeed(OAuthAdapter, {
	Begin: () => Effect.fail(new OAuthError({ operation: "unavailable" })),
	Complete: () => Effect.fail(new OAuthError({ operation: "unavailable" })),
	Refresh: () => Effect.fail(new OAuthError({ operation: "unavailable" })),
	Revoke: () => Effect.fail(new OAuthError({ operation: "unavailable" })),
	Status: () => Effect.fail(new OAuthError({ operation: "unavailable" })),
});
