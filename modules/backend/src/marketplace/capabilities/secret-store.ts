import { Context, Data, Effect, Layer, Redacted } from "effect";

/** An opaque name for a secret held outside Marketplace persistence. */
export type SecretReference = string & { readonly SecretReference: unique symbol };

/** Creates an opaque secret reference after rejecting blank names. */
export const secret_reference = (value: string): Effect.Effect<SecretReference, SecretStoreError> =>
	value.trim().length > 0
		? Effect.succeed(value as SecretReference)
		: Effect.fail(new SecretStoreError({ operation: "reference" }));

/** Reports a safe secret-store failure. Values are deliberately never attached. */
export class SecretStoreError extends Data.TaggedError("SecretStoreError")<{
	readonly operation: "get" | "reference" | "unavailable";
}> {}

/** Resolves opaque references from an OS-backed secret facility. */
export class SecretStore extends Context.Service<
	SecretStore,
	{
		readonly Get: (
			reference: SecretReference,
		) => Effect.Effect<Redacted.Redacted<string>, SecretStoreError>;
	}
>()("Artisan/Marketplace/SecretStore") {}

/** Default-deny store used until desktop composition supplies a secure vault. */
export const EmptySecretStoreLive = Layer.succeed(SecretStore, {
	Get: () => Effect.fail(new SecretStoreError({ operation: "unavailable" })),
});
