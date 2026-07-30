import { Effect, Layer } from "effect";
import { describe, expect, it } from "vitest";

import {
	WorkspaceFilesystemNotFoundError,
	WorkspaceFilesystemRegistrationError,
	WorkspaceFilesystemRegistry,
	type WorkspaceFilesystem,
} from "../../modules/backend/src/filesystem/workspace-filesystem-registry";
import {
	WorkspaceFileDiscovery,
	WorkspaceFileDiscoveryLive,
} from "../../modules/backend/src/workspace/files/discovery";

const filesystem = {
	List: (path = ".") =>
		Effect.succeed(
			path === "."
				? [
						{
							created_at: "2026-01-01T00:00:00.000Z",
							kind: "directory" as const,
							mode: 0,
							modified_at: "2026-01-01T00:00:00.000Z",
							path: "src",
							size: 0,
						},
						{
							created_at: "2026-01-01T00:00:00.000Z",
							kind: "directory" as const,
							mode: 0,
							modified_at: "2026-01-01T00:00:00.000Z",
							path: "src2",
							size: 0,
						},
						{
							created_at: "2026-01-01T00:00:00.000Z",
							kind: "directory" as const,
							mode: 0,
							modified_at: "2026-01-01T00:00:00.000Z",
							path: "vendor",
							size: 0,
						},
					]
				: path === "src"
					? [
							{
								created_at: "2026-01-01T00:00:00.000Z",
								kind: "file" as const,
								mode: 0,
								modified_at: "2026-01-01T00:00:00.000Z",
								path: "src/app.ts",
								size: 12,
							},
						]
					: [
							{
								created_at: "2026-01-01T00:00:00.000Z",
								kind: "file" as const,
								mode: 0,
								modified_at: "2026-01-01T00:00:00.000Z",
								path: "vendor/large.js",
								size: 1,
							},
						],
		),
} as unknown as WorkspaceFilesystem;

const registry = Layer.succeed(WorkspaceFilesystemRegistry, {
	Authorize: () => Effect.die("not used"),
	Get: (workspace_id: string) =>
		workspace_id === "workspace"
			? Effect.succeed({ filesystem, workspace_id })
			: Effect.fail(new WorkspaceFilesystemNotFoundError({ workspace_id })),
	ListWorkspaceIds: Effect.succeed(["workspace"]),
	Reconcile: () => Effect.succeed([]),
	/** Discovery never registers; the fake refuses rather than pretending to. */
	Register: () =>
		Effect.fail(
			new WorkspaceFilesystemRegistrationError({ message: "registration is not under test" }),
		),
});

describe("WorkspaceFileDiscovery", () => {
	it("keeps discovery root-confined and prunes non-prefix subtrees", async () => {
		const service = await Effect.runPromise(
			Effect.service(WorkspaceFileDiscovery).pipe(
				Effect.provide(WorkspaceFileDiscoveryLive.pipe(Layer.provide(registry))),
			),
		);
		const result = await Effect.runPromise(
			service.Discover({ limit: 10, prefix: "src", workspace_id: "workspace" }),
		);
		expect(result.entries.map((entry) => entry.path)).toEqual(["src", "src/app.ts"]);
	});

	it("rejects unknown workspaces for language capability projections", async () => {
		const service = await Effect.runPromise(
			Effect.service(WorkspaceFileDiscovery).pipe(
				Effect.provide(WorkspaceFileDiscoveryLive.pipe(Layer.provide(registry))),
			),
		);
		await expect(
			Effect.runPromise(service.LanguageCapabilities({ workspace_id: "missing" })),
		).rejects.toMatchObject({ reason: "unavailable" });
	});

	it("reports traversal-cap exhaustion truthfully when a late cursor filters every path", async () => {
		const many_files = Array.from({ length: 10_001 }, (_, index) => ({
			created_at: "2026-01-01T00:00:00.000Z",
			kind: "file" as const,
			mode: 0,
			modified_at: "2026-01-01T00:00:00.000Z",
			path: `file-${String(index).padStart(5, "0")}.ts`,
			size: 1,
		}));
		const bounded_filesystem = {
			...filesystem,
			List: () => Effect.succeed(many_files),
		} as unknown as WorkspaceFilesystem;
		const bounded_registry = Layer.succeed(WorkspaceFilesystemRegistry, {
			Authorize: () => Effect.die("not used"),
			Get: (workspace_id: string) =>
				Effect.succeed({ filesystem: bounded_filesystem, workspace_id }),
			ListWorkspaceIds: Effect.succeed(["workspace"]),
			Reconcile: () => Effect.succeed([]),
			Register: () =>
				Effect.fail(
					new WorkspaceFilesystemRegistrationError({
						message: "registration is not under test",
					}),
				),
		});
		const service = await Effect.runPromise(
			Effect.service(WorkspaceFileDiscovery).pipe(
				Effect.provide(WorkspaceFileDiscoveryLive.pipe(Layer.provide(bounded_registry))),
			),
		);
		const result = await Effect.runPromise(
			service.Discover({ after_path: "z", workspace_id: "workspace" }),
		);

		expect(result.entries).toEqual([]);
		expect(result.truncated).toBe(true);
	});
});
