import { createHash } from "node:crypto";

import { Context, Effect, Layer, Option } from "effect";

import { normalize_global_guidance_content, type GlobalGuidanceProvider } from "@artisan/protocol";

import { GuidanceFileStore, type GuidanceFile, type GuidanceFileStoreFailure } from "./file-store";

/** Describes the observable state of one provider-native guidance file. */
export type GuidanceDiscovery =
	| {
			readonly _tag: "Absent";
			readonly path: string;
	  }
	| {
			readonly _tag: "Present";
			readonly content: string;
			readonly hash: string;
			readonly modified_at: string;
			readonly path: string;
	  }
	| {
			readonly _tag: "ReadFailed";
			readonly path: string;
	  };

/** Supplies filesystem discovery for a provider with native global guidance. */
export interface NativeGuidanceProviderAdapter {
	readonly Discover: Effect.Effect<GuidanceDiscovery>;
	readonly mode: "native_file";
	readonly provider: GlobalGuidanceProvider;
}

/** Marks a provider whose guidance is injected into each run by its Engine. */
export interface RuntimeGuidanceProviderAdapter {
	readonly mode: "runtime";
	readonly provider: GlobalGuidanceProvider;
}

/** Marks a provider that cannot consume Artisan global guidance. */
export interface UnsupportedGuidanceProviderAdapter {
	readonly mode: "unsupported";
	readonly provider: GlobalGuidanceProvider;
}

/** Defines every provider guidance integration mode understood by the backend. */
export type GuidanceProviderAdapter =
	| NativeGuidanceProviderAdapter
	| RuntimeGuidanceProviderAdapter
	| UnsupportedGuidanceProviderAdapter;

/** Owns the provider adapters enabled for one backend runtime. */
export class GuidanceProviderRegistry extends Context.Service<
	GuidanceProviderRegistry,
	{
		readonly Providers: ReadonlyArray<GuidanceProviderAdapter>;
	}
>()("Artisan/GuidanceProviderRegistry") {}

/** Supplies a provider-neutral registry for portable and test compositions. */
export const EmptyGuidanceProviderRegistryLive = Layer.succeed(GuidanceProviderRegistry, {
	Providers: [],
});

/** Configures native user-guidance paths for providers present in the desktop runtime. */
export interface PlatformGuidanceProviderOptions {
	readonly codex_agents_path: string;
	readonly codex_override_path: string;
}

interface GuidanceReader {
	readonly Read: (
		path: string,
	) => Effect.Effect<Option.Option<GuidanceFile>, GuidanceFileStoreFailure>;
}

/** Normalizes provider files into Artisan's canonical text representation. */
export function normalize_guidance_content(content: string) {
	return normalize_global_guidance_content(content);
}

/** Produces the stable content identity stored instead of private guidance text. */
export function guidance_hash(content: string) {
	return createHash("sha256").update(normalize_guidance_content(content)).digest("hex");
}

function discover_file(files: GuidanceReader, path: string, empty_is_absent = false) {
	return files.Read(path).pipe(
		Effect.map((file) =>
			Option.match(file, {
				onNone: () => ({ _tag: "Absent" as const, path }),
				onSome: ({ content, modified_at }) => {
					const normalized = normalize_guidance_content(content);

					return empty_is_absent && normalized.length === 0
						? { _tag: "Absent" as const, path }
						: {
								_tag: "Present" as const,
								content: normalized,
								hash: guidance_hash(normalized),
								modified_at,
								path,
							};
				},
			}),
		),
		Effect.catch(() => Effect.succeed({ _tag: "ReadFailed" as const, path })),
	);
}

/** Creates the configured provider registry. */
export function make_guidance_provider_registry_layer(
	providers: ReadonlyArray<GuidanceProviderAdapter>,
) {
	return Layer.succeed(GuidanceProviderRegistry, { Providers: providers });
}

/** Creates the Codex-only registry used by the desktop production composition root. */
export function make_platform_guidance_provider_registry_layer(
	options: PlatformGuidanceProviderOptions,
) {
	return Layer.effect(
		GuidanceProviderRegistry,
		Effect.gen(function* () {
			return {
				Providers: [
					yield* make_codex_guidance_adapter(
						options.codex_override_path,
						options.codex_agents_path,
					),
				],
			};
		}),
	);
}

/** Creates a provider entry whose canonical guidance is supplied out of band per run. */
export function make_runtime_guidance_adapter(
	provider: GlobalGuidanceProvider,
): RuntimeGuidanceProviderAdapter {
	return { mode: "runtime", provider };
}

/** Creates a provider entry that reports guidance as unsupported. */
export function make_unsupported_guidance_adapter(
	provider: GlobalGuidanceProvider,
): UnsupportedGuidanceProviderAdapter {
	return { mode: "unsupported", provider };
}

/** Creates the native Codex adapter with override-file precedence. */
export function make_codex_guidance_adapter(override_path: string, agents_path: string) {
	return Effect.gen(function* () {
		const files = yield* GuidanceFileStore;
		const Discover = discover_file(files, override_path, true).pipe(
			Effect.flatMap((override) =>
				override._tag === "Absent"
					? discover_file(files, agents_path)
					: Effect.succeed(override),
			),
		);

		return {
			Discover,
			mode: "native_file" as const,
			provider: "codex" as const,
		} satisfies NativeGuidanceProviderAdapter;
	});
}
