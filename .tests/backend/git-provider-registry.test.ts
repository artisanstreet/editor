import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import {
	GitProvider,
	normalize_git_provider_host,
	type GitProviderInspection,
} from "../../modules/backend/src/git-provider/git-provider";
import {
	GitProviderRegistry,
	make_git_provider_registry_layer,
	type GitProviderRegistration,
} from "../../modules/backend/src/git-provider/git-provider-registry";

function make_provider(
	provider_id: string,
	authentication: GitProviderInspection["authentication"] = [],
) {
	return {
		Descriptor: {
			capabilities: [
				{ _tag: "available" as const, capability: "discover_repositories" as const },
			],
			display_name: provider_id,
			provider_id,
		},
		DiscoverRepositories: () => Effect.die("Discovery is outside registry resolution tests"),
		Inspect: Effect.succeed({
			authentication,
			installation: {
				_tag: "available" as const,
				executable_path: "C:\\Program Files\\GitHub CLI\\gh.exe",
				version: "1.0.0",
			},
		}),
	} satisfies typeof GitProvider.Service;
}

function make_registry(registrations: ReadonlyArray<GitProviderRegistration>) {
	return Effect.service(GitProviderRegistry).pipe(
		Effect.provide(make_git_provider_registry_layer(registrations)),
	);
}

describe("GitProviderRegistry", () => {
	it("normalizes only host[:port] inputs", () => {
		expect(normalize_git_provider_host("GITHUB.COM")).toBe("github.com");
		expect(normalize_git_provider_host("m\u00fcnich.example:8443")).toBe(
			"xn--mnich-kva.example:8443",
		);
		expect(normalize_git_provider_host("https://github.com")).toBeUndefined();
		expect(normalize_git_provider_host("github.com/path")).toBeUndefined();
	});

	it("resolves normalized static hosts to their explicit provider", async () => {
		const registry = await Effect.runPromise(
			make_registry([{ hosts: ["github.com"], provider: make_provider("github") }]),
		);

		await expect(Effect.runPromise(registry.ResolveHost("GITHUB.COM"))).resolves.toEqual({
			_tag: "resolved",
			host: "github.com",
			provider_id: "github",
		});
	});

	it("resolves only exact dynamically inspected enterprise hosts", async () => {
		const registry = await Effect.runPromise(
			make_registry([
				{
					hosts: ["github.com"],
					provider: make_provider("github", [
						{
							accounts: [{ _tag: "authenticated", account_login: "sander" }],
							active_account: { _tag: "selected", account_login: "sander" },
							host: "git.example.test",
						},
					]),
				},
			]),
		);

		await expect(
			Effect.runPromise(registry.ResolveHost("git.example.test")),
		).resolves.toMatchObject({
			_tag: "resolved",
			provider_id: "github",
		});
		await expect(
			Effect.runPromise(registry.ResolveHost("api.git.example.test")),
		).resolves.toEqual({
			_tag: "unsupported",
			host: "api.git.example.test",
		});
	});

	it("keeps unknown valid hosts unsupported", async () => {
		const registry = await Effect.runPromise(make_registry([]));

		await expect(Effect.runPromise(registry.ResolveHost("gitlab.com"))).resolves.toEqual({
			_tag: "unsupported",
			host: "gitlab.com",
		});
	});

	it("rejects duplicate provider IDs and static hosts", async () => {
		const duplicate_id = Effect.runPromise(
			make_registry([
				{ hosts: ["one.example.test"], provider: make_provider("github") },
				{ hosts: ["two.example.test"], provider: make_provider("github") },
			]),
		);
		const duplicate_host = Effect.runPromise(
			make_registry([
				{ hosts: ["github.com"], provider: make_provider("github") },
				{ hosts: ["GITHUB.COM"], provider: make_provider("gitlab") },
			]),
		);

		await expect(duplicate_id).rejects.toMatchObject({ reason: "duplicate_provider_id" });
		await expect(duplicate_host).rejects.toMatchObject({ reason: "duplicate_host" });
	});

	it("rejects invalid registered and requested hosts", async () => {
		await expect(
			Effect.runPromise(
				make_registry([
					{ hosts: ["https://github.com"], provider: make_provider("github") },
				]),
			),
		).rejects.toMatchObject({ reason: "invalid_host" });

		const registry = await Effect.runPromise(make_registry([]));

		await expect(
			Effect.runPromise(registry.ResolveHost("github.com/path")),
		).rejects.toMatchObject({
			reason: "invalid_host",
		});
	});
});
