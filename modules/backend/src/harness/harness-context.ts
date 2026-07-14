import { Context, Effect, Layer } from "effect";

import type { EngineDescriptor, EngineHarnessContext } from "@artisan/engines";

/** Stable version identifier for Artisan's hosted review and CI wait policy. */
export const artisan_harness_context_version = "v1";

/** Describes whether one engine can enforce the Artisan harness policy for a run. */
export type ArtisanHarnessContextResolution =
	| { readonly _tag: "available"; readonly context: EngineHarnessContext }
	| { readonly _tag: "unavailable"; readonly reason: string };

/** Resolves the backend-owned policy only for engines with a strong instruction channel. */
export class ArtisanHarnessContext extends Context.Service<
	ArtisanHarnessContext,
	{
		readonly ResolveForEngine: (
			descriptor: EngineDescriptor,
		) => Effect.Effect<ArtisanHarnessContextResolution>;
	}
>()("Artisan/HarnessContext") {}

const content = [
	"Artisan monitors hosted review and CI outside model runs.",
	"Never sleep, watch, shell-loop, repeatedly poll, or use gh run watch.",
	"When hosted review or CI is the only remaining dependency, call await_git_provider once with the exact pull request, head commit, and gates.",
	"Claim automatic invocation only after the wait has been durably accepted.",
	"After acceptance, briefly inform the user and end the run.",
	"Artisan later resumes the native conversation or starts a linked follow-up run.",
].join(" ");

/** Provides the deterministic V1 Artisan harness policy without external dependencies. */
export const ArtisanHarnessContextLive = Layer.succeed(ArtisanHarnessContext, {
	ResolveForEngine: (descriptor) =>
		Effect.succeed(
			descriptor.capabilities.harness_context.state !== "unsupported"
				? {
						_tag: "available" as const,
						context: { content, version: artisan_harness_context_version },
					}
				: {
						_tag: "unavailable" as const,
						reason:
							descriptor.capabilities.harness_context.reason ??
							"The engine does not declare a strong per-run harness instruction channel.",
					},
		),
});
