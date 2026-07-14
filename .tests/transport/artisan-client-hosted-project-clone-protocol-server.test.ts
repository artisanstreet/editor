import { fileURLToPath } from "node:url";

import { NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import { Effect, FileSystem, Layer, Path } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	GitProvider,
	GitProviderError,
	HostedProjectCloneCoordinator,
	make_backend_runtime,
	make_git_provider_registry_layer,
	make_hosted_project_clone_destination_layer,
	ProtocolServer,
	type GitProviderCloneRequest,
} from "@artisan/backend";

import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";

import {
	make_transport_test_harness_with_protocol_server,
	wait_for,
} from "./message-channel-harness";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const platform_layer = Layer.mergeAll(NodeFileSystem.layer, NodePath.layer);
const directories: Array<string> = [];

interface FakeProviderState {
	clone_calls: number;
	prepare_calls: number;
}

const TestPlatform = Effect.all({
	file_system: FileSystem.FileSystem,
	path_service: Path.Path,
}).pipe(Effect.provide(platform_layer));

async function make_temporary_root() {
	const { file_system } = await Effect.runPromise(TestPlatform);
	const root = await Effect.runPromise(
		file_system.makeTempDirectory({ prefix: "artisan-hosted-clone-protocol-" }),
	);

	directories.push(root);

	return root;
}

function repository() {
	return {
		archived: false,
		clone_url: "https://github.com/artisan/editor.git",
		default_branch: { _tag: "known" as const, name: "main" },
		identity: {
			host: "github.com",
			name: "editor",
			owner: "artisan",
			provider_id: "github",
		},
		origin: {
			native_id: "repository_1",
			provider_id: "github",
			resource_kind: "repository" as const,
		},
		viewer_permission: "write" as const,
		visibility: "private" as const,
		web_url: "https://github.com/artisan/editor",
	};
}

function clone_request(destination_path: string) {
	return {
		command_id: "clone_protocol_restart",
		destination_path,
		repository: repository(),
		selection: {
			account_login: "sander",
			host: "github.com",
			provider_id: "github",
		},
		thread_id: "thread_clone_protocol",
	};
}

function make_metadata_layer(instance_id: string) {
	let next_id = 0;
	let next_time = Date.parse("2026-07-14T12:00:00.000Z");

	return Layer.succeed(RuntimeMetadata, {
		instance_id,
		MakeId: (prefix) => Effect.sync(() => `${prefix}_${instance_id}_${++next_id}`),
		Now: Effect.sync(() => new Date(next_time++).toISOString()),
	});
}

function make_provider(
	state: FakeProviderState,
	file_system: FileSystem.FileSystem,
	path_service: Path.Path,
) {
	return {
		Clone: (input) =>
			Effect.gen(function* () {
				state.clone_calls += 1;

				yield* file_system
					.makeDirectory(path_service.join(input.destination.canonical_root, ".git"))
					.pipe(
						Effect.mapError(
							() =>
								new GitProviderError({
									host: "github.com",
									operation: "clone_repository",
									provider_id: "github",
									reason: "clone_failed",
									retryable: false,
								}),
						),
					);

				return {
					canonical_root: input.destination.canonical_root,
					output_complete: true,
					repository: input.preparation.repository,
					type: "cloned" as const,
				};
			}),
		Descriptor: {
			capabilities: [{ _tag: "available" as const, capability: "clone_repository" as const }],
			display_name: "GitHub",
			provider_id: "github",
		},
		DiscoverRepositories: () => Effect.die("Discovery is outside clone protocol tests"),
		Inspect: Effect.die("Inspection is outside clone protocol tests"),
		PrepareClone: (input: GitProviderCloneRequest) =>
			Effect.sync(() => {
				state.prepare_calls += 1;

				return { repository: input.repository, selection: input.selection };
			}),
	} satisfies typeof GitProvider.Service;
}

async function start_stack(options: {
	readonly database_path: string;
	readonly instance_id: string;
	readonly projects_root: string;
	readonly provider: typeof GitProvider.Service;
}) {
	const destination = make_hosted_project_clone_destination_layer({
		projects_root: options.projects_root,
	}).pipe(Layer.provideMerge(NodeFileSystem.layer), Layer.provideMerge(NodePath.layer));
	const runtime = make_backend_runtime({
		database_path: options.database_path,
		git_provider_registry: make_git_provider_registry_layer([
			{ hosts: ["github.com"], provider: options.provider },
		]),
		hosted_project_clone_destination: destination,
		migrations_path,
		runtime_metadata: make_metadata_layer(options.instance_id),
	});
	const protocol_server = await runtime.runPromise(ProtocolServer);
	const coordinator = await runtime.runPromise(HostedProjectCloneCoordinator);
	const harness = await make_transport_test_harness_with_protocol_server(protocol_server, {
		client: { reconnect_delay_ms: 5 },
	});

	return { coordinator, harness, runtime };
}

afterEach(async () => {
	const { file_system } = await Effect.runPromise(TestPlatform);
	const cleanup = directories.splice(0);

	await Effect.runPromise(
		Effect.forEach(cleanup, (directory) => file_system.remove(directory, { recursive: true }), {
			discard: true,
		}),
	);
});

describe("ArtisanClient hosted clone with the backend ProtocolServer", () => {
	it("survives backend restart and renderer reconnect without repeating the clone", async () => {
		const root = await make_temporary_root();
		const { file_system, path_service } = await Effect.runPromise(TestPlatform);
		const database_path = path_service.join(root, "artisan.db");
		const projects_root = path_service.join(root, "projects");
		const destination_path = path_service.join(projects_root, "editor");
		const state = { clone_calls: 0, prepare_calls: 0 };
		const provider = make_provider(state, file_system, path_service);
		let first: Awaited<ReturnType<typeof start_stack>> | undefined = await start_stack({
			database_path,
			instance_id: "before_restart",
			projects_root,
			provider,
		});
		let second: Awaited<ReturnType<typeof start_stack>> | undefined;

		await Effect.runPromise(file_system.makeDirectory(destination_path, { recursive: true }));

		try {
			await Effect.runPromise(
				first.harness.client.Command({
					command_id: "create_clone_protocol_thread",
					payload: { title: "Clone through protocol", type: "thread.create" },
					thread_id: "thread_clone_protocol",
				}),
			);
			const requested_receipt = await Effect.runPromise(
				first.harness.client.RequestHostedProjectClone(clone_request(destination_path)),
			);
			const requested = await Effect.runPromise(
				first.harness.client.GetHostedProjectCloneApproval({
					approval_id: "hosted_project_clone:clone_protocol_restart",
					thread_id: "thread_clone_protocol",
				}),
			);

			expect(requested_receipt.status).toBe("accepted");
			expect(requested.approval.state).toBe("requested");
			expect(requested.approval.repository).not.toHaveProperty("clone_url");
			expect(state).toEqual({ clone_calls: 0, prepare_calls: 1 });

			await first.harness.dispose();
			await first.runtime.dispose();
			first = undefined;

			second = await start_stack({
				database_path,
				instance_id: "after_restart",
				projects_root,
				provider,
			});
			const restored = await Effect.runPromise(
				second.harness.client.GetHostedProjectCloneApproval({
					approval_id: "hosted_project_clone:clone_protocol_restart",
					thread_id: "thread_clone_protocol",
				}),
			);

			expect(restored.approval.state).toBe("requested");

			const approval_receipt = await Effect.runPromise(
				second.harness.client.RespondHostedProjectCloneApproval({
					approval_id: "hosted_project_clone:clone_protocol_restart",
					approved: true,
					command_id: "clone_protocol_approval",
					thread_id: "thread_clone_protocol",
				}),
			);

			await Effect.runPromise(second.coordinator.AwaitIdle);
			second.harness.close_current_connection();
			const applied_promise = Effect.runPromise(
				second.harness.client.GetHostedProjectCloneApproval({
					approval_id: "hosted_project_clone:clone_protocol_restart",
					thread_id: "thread_clone_protocol",
				}),
			);

			await wait_for(() => second!.harness.connector_snapshot().connections >= 2);

			const applied = await applied_promise;

			expect(approval_receipt.status).toBe("accepted");
			expect(applied.approval).toMatchObject({
				attachment: "attached",
				state: "applied",
			});
			expect(state).toEqual({ clone_calls: 1, prepare_calls: 1 });
			expect(await Effect.runPromise(file_system.readDirectory(destination_path))).toEqual([
				".git",
			]);
		} finally {
			await first?.harness.dispose();
			await first?.runtime.dispose();
			await second?.harness.dispose();
			await second?.runtime.dispose();
		}
	});
});
