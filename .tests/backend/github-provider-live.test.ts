import { NodeCrypto, NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import { Effect, Layer } from "effect";
import { describe, expect, it } from "vitest";

import { GitProvider } from "../../modules/backend/src/git-provider/git-provider";
import { make_github_cli_layer } from "../../modules/backend/src/git-provider/github/github-cli";
import {
	make_node_github_cli_executable_layer,
	make_node_github_cli_git_executable_layer,
} from "../../modules/backend/src/git-provider/github/github-cli-executable";
import { make_github_provider_layer } from "../../modules/backend/src/git-provider/github/github-provider";
import { NodeProcessRunnerLive } from "../../modules/backend/src/git/node-process-runner";

const run_live = process.env.ARTISAN_RUN_GITHUB_PROVIDER_LIVE === "1";

describe.skipIf(!run_live)("GitHubProvider live", () => {
	it("inspects the installed gh session and reads one canonical repository page", async () => {
		const cwd = process.cwd();
		const executable = make_node_github_cli_executable_layer({ cwd });
		const git_executable = make_node_github_cli_git_executable_layer({ cwd });
		const cli = make_github_cli_layer({ cwd }).pipe(
			Layer.provideMerge(NodeProcessRunnerLive),
			Layer.provideMerge(NodeCrypto.layer),
			Layer.provideMerge(NodeFileSystem.layer),
			Layer.provideMerge(NodePath.layer),
			Layer.provideMerge(executable),
			Layer.provideMerge(git_executable),
		);
		const provider = await Effect.runPromise(
			Effect.service(GitProvider).pipe(
				Effect.provide(make_github_provider_layer().pipe(Layer.provide(cli))),
			),
		);
		const inspection = await Effect.runPromise(provider.Inspect);

		expect(inspection.installation._tag).toBe("available");

		const authentication = inspection.authentication.find(
			(host) => host.active_account._tag === "selected",
		);

		expect(authentication).toBeDefined();
		if (authentication?.active_account._tag !== "selected") {
			return;
		}

		const page = await Effect.runPromise(
			provider.DiscoverRepositories({
				page_size: 1,
				position: { _tag: "first" },
				scope: { _tag: "account" },
				selection: {
					account_login: authentication.active_account.account_login,
					host: authentication.host,
					provider_id: "github",
				},
			}),
		);

		expect(page.repositories.length).toBeLessThanOrEqual(1);
		expect(
			page.repositories.every(
				(repository) =>
					repository.identity.provider_id === "github" &&
					repository.identity.host === authentication.host,
			),
		).toBe(true);
	});
});
