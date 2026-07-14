import { Context, Data, Effect, Layer, Schema } from "effect";

import {
	GitProvider,
	GitProviderDescriptor,
	GitProviderHost,
	GitProviderInspection,
	GitProviderId,
	normalize_git_provider_host,
} from "./git-provider";

/** Reports invalid provider registration or an ambiguous dynamic host ownership claim. */
export class GitProviderRegistryError extends Data.TaggedError("GitProviderRegistryError")<{
	readonly reason:
		| "ambiguous_host"
		| "duplicate_host"
		| "duplicate_provider_id"
		| "invalid_host"
		| "invalid_provider";
}> {}

/** Resolves a canonical host to its provider ID or retains unsupported-host behavior. */
export const GitProviderHostResolution = Schema.Union([
	Schema.Struct({
		_tag: Schema.Literal("resolved"),
		host: GitProviderHost,
		provider_id: GitProviderId,
	}),
	Schema.Struct({
		_tag: Schema.Literal("unsupported"),
		host: GitProviderHost,
	}),
]);

export type GitProviderHostResolution = typeof GitProviderHostResolution.Type;

/** Registers a provider service alongside the static hosts that always belong to it. */
export interface GitProviderRegistration {
	readonly hosts: ReadonlyArray<string>;
	readonly provider: typeof GitProvider.Service;
}

/** Owns explicit hosted-forge registrations and exact host-to-provider resolution. */
export class GitProviderRegistry extends Context.Service<
	GitProviderRegistry,
	{
		readonly Get: (
			provider_id: string,
		) => Effect.Effect<typeof GitProvider.Service, GitProviderRegistryError>;
		readonly ResolveHost: (
			host: string,
		) => Effect.Effect<
			GitProviderHostResolution,
			GitProviderRegistryError | import("./git-provider").GitProviderError
		>;
	}
>()("Artisan/GitProviderRegistry") {}

interface RegisteredGitProvider {
	readonly descriptor: typeof GitProviderDescriptor.Type;
	readonly hosts: ReadonlyArray<string>;
	readonly provider: typeof GitProvider.Service;
}

function registry_error(reason: GitProviderRegistryError["reason"]) {
	return new GitProviderRegistryError({ reason });
}

function resolve_registered_host(
	host: string,
	registrations: ReadonlyArray<RegisteredGitProvider>,
) {
	const static_matches = registrations.filter((registration) =>
		registration.hosts.includes(host),
	);

	if (static_matches.length > 0) {
		return Effect.succeed(static_matches[0]!);
	}

	return Effect.forEach(registrations, (registration) =>
		registration.provider.Inspect.pipe(
			Effect.flatMap((inspection) =>
				Schema.decodeUnknownEffect(GitProviderInspection, {
					onExcessProperty: "error",
				})(inspection).pipe(
					Effect.mapError(() => registry_error("invalid_provider")),
					Effect.map((decoded) => ({
						registration,
						matches: decoded.authentication.some(
							(authentication) => authentication.host === host,
						),
					})),
				),
			),
		),
	).pipe(
		Effect.flatMap((inspections) => {
			const matches = inspections.filter((inspection) => inspection.matches);

			return matches.length > 1
				? Effect.fail(registry_error("ambiguous_host"))
				: Effect.succeed(matches[0]?.registration);
		}),
	);
}

function BuildGitProviderRegistry(registrations: ReadonlyArray<GitProviderRegistration>) {
	return Effect.gen(function* () {
		const registered = yield* Effect.forEach(registrations, (registration) =>
			Effect.gen(function* () {
				const descriptor = yield* Schema.decodeUnknownEffect(GitProviderDescriptor, {
					onExcessProperty: "error",
				})(registration.provider.Descriptor).pipe(
					Effect.mapError(() => registry_error("invalid_provider")),
				);
				const hosts = registration.hosts.map(normalize_git_provider_host);

				if (hosts.some((host) => host === undefined)) {
					return yield* Effect.fail(registry_error("invalid_host"));
				}
				const normalized_hosts = hosts.filter((host): host is string => host !== undefined);

				return {
					descriptor,
					hosts: normalized_hosts,
				} satisfies Omit<RegisteredGitProvider, "provider">;
			}),
		);
		const provider_ids = registered.map(({ descriptor }) => descriptor.provider_id);

		if (new Set(provider_ids).size !== provider_ids.length) {
			return yield* Effect.fail(registry_error("duplicate_provider_id"));
		}

		const static_hosts = registered.flatMap(({ hosts }) => hosts);

		if (new Set(static_hosts).size !== static_hosts.length) {
			return yield* Effect.fail(registry_error("duplicate_host"));
		}

		const providers = registrations.map((registration, index) => ({
			...registered[index]!,
			provider: registration.provider,
		}));
		const by_provider_id = new Map(
			providers.map((registration) => [registration.descriptor.provider_id, registration]),
		);
		const Get = (provider_id: string) => {
			const provider = by_provider_id.get(provider_id);

			return provider === undefined
				? Effect.fail(registry_error("invalid_provider"))
				: Effect.succeed(provider.provider);
		};
		const ResolveHost = (input: string) => {
			const host = normalize_git_provider_host(input);

			if (host === undefined) {
				return Effect.fail(registry_error("invalid_host"));
			}

			return resolve_registered_host(host, providers).pipe(
				Effect.map(
					(provider): GitProviderHostResolution =>
						provider === undefined
							? { _tag: "unsupported", host }
							: {
									_tag: "resolved",
									host,
									provider_id: provider.descriptor.provider_id,
								},
				),
			);
		};

		return { Get, ResolveHost };
	});
}

/** Builds a provider registry from explicit provider services and static host ownership. */
export function make_git_provider_registry_layer(
	registrations: ReadonlyArray<GitProviderRegistration>,
) {
	return Layer.effect(GitProviderRegistry, BuildGitProviderRegistry(registrations));
}

/** Supplies a portable provider registry with no hosted-forge integrations. */
export const EmptyGitProviderRegistryLive = make_git_provider_registry_layer([]);
