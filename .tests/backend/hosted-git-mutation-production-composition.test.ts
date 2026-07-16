import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	AgentGraphOrchestrator,
	AgentOrchestrator,
	ExternalWaitDispatcher,
	HostedGitMutationCoordinator,
	HostedGitMutationRepository,
	HostedProjectCloneCoordinator,
	make_backend_runtime,
	PreviewBrowserLifecycle,
	TerminalSessionService,
	WorkspaceGitCheckoutCoordinator,
	WorkspaceGitFetchService,
	WorkspaceGitMutationCoordinator,
	WorkspaceReplaceApprovalCoordinator,
} from "../../modules/backend/src/index";
import {
	ThreadResourceQuiescer,
	ThreadResourceQuiescerLive,
} from "../../modules/backend/src/threads/thread-resource-quiescer";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-hosted-git-mutation-composition-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

function make_quiesce_layer<Service>(
	service: { readonly key: string },
	calls: Array<string>,
): Layer.Layer<Service> {
	return Layer.succeed(
		service as never,
		{
			QuiesceThread: (thread_id: string) =>
				Effect.sync(() => {
					calls.push(`${service.key}:${thread_id}`);
				}),
		} as never,
	);
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("hosted Git mutation production composition", () => {
	it("acquires the repository and coordinator from the complete backend runtime", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			migrations_path,
		});

		try {
			const services = await runtime.runPromise(
				Effect.all([HostedGitMutationCoordinator, HostedGitMutationRepository]),
			);

			expect(services[0].Recover).toBeTypeOf("object");
			expect(services[1].AbandonOwnedExecutions).toBeTypeOf("object");
		} finally {
			await runtime.dispose();
		}
	});

	it("quiesces hosted mutation dispatch with the other thread-owned resources", async () => {
		const calls: Array<string> = [];
		const services = Layer.mergeAll(
			make_quiesce_layer<typeof ExternalWaitDispatcher.Service>(
				ExternalWaitDispatcher,
				calls,
			),
			make_quiesce_layer<typeof AgentGraphOrchestrator.Service>(
				AgentGraphOrchestrator,
				calls,
			),
			make_quiesce_layer<typeof HostedGitMutationCoordinator.Service>(
				HostedGitMutationCoordinator,
				calls,
			),
			make_quiesce_layer<typeof HostedProjectCloneCoordinator.Service>(
				HostedProjectCloneCoordinator,
				calls,
			),
			make_quiesce_layer<typeof AgentOrchestrator.Service>(AgentOrchestrator, calls),
			make_quiesce_layer<typeof PreviewBrowserLifecycle.Service>(
				PreviewBrowserLifecycle,
				calls,
			),
			make_quiesce_layer<typeof TerminalSessionService.Service>(
				TerminalSessionService,
				calls,
			),
			make_quiesce_layer<typeof WorkspaceGitCheckoutCoordinator.Service>(
				WorkspaceGitCheckoutCoordinator,
				calls,
			),
			make_quiesce_layer<typeof WorkspaceGitFetchService.Service>(
				WorkspaceGitFetchService,
				calls,
			),
			make_quiesce_layer<typeof WorkspaceGitMutationCoordinator.Service>(
				WorkspaceGitMutationCoordinator,
				calls,
			),
			make_quiesce_layer<typeof WorkspaceReplaceApprovalCoordinator.Service>(
				WorkspaceReplaceApprovalCoordinator,
				calls,
			),
		);
		const quiescer = ThreadResourceQuiescerLive.pipe(
			Layer.provide(services as never),
		) as Layer.Layer<ThreadResourceQuiescer>;
		const runtime = ManagedRuntime.make(quiescer);

		try {
			await runtime.runPromise(
				ThreadResourceQuiescer.pipe(
					Effect.flatMap((quiescer) => quiescer.Quiesce("thread_1")),
				),
			);

			expect(calls).toContain("Artisan/HostedGitMutationCoordinator:thread_1");
		} finally {
			await runtime.dispose();
		}
	});
});
