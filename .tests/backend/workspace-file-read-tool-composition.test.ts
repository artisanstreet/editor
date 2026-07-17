import { createHash } from "node:crypto";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Layer } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { make_backend_runtime } from "@artisan/backend";
import { type ToolDescriptorReference, workspace_text_maximum_bytes } from "@artisan/protocol";

import { WorkspaceBoundedRegularFileStoreRegistry } from "../../modules/backend/src/filesystem/workspace-bounded-regular-file-store-registry";
import { ToolRegistry } from "../../modules/backend/src/tool-control/tool-registry";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];

function tool_invocation(
	context: { agent_id: string; run_id: string; thread_id: string; workspace_id: string },
	tool: ToolDescriptorReference,
) {
	return { context, invocation_id: "invocation", tool };
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("WorkspaceFileReadTool production composition", () => {
	it("registers and executes the file reader in the default backend tool registry", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-workspace-read-tool-"));
		const content = new TextEncoder().encode("production");
		const reads: Array<{ maximum_bytes: number; path: string }> = [];

		temporary_directories.push(directory);

		const workspace_registry = Layer.succeed(WorkspaceBoundedRegularFileStoreRegistry, {
			Authorize: () => Effect.die("unused"),
			Get: (workspace_id: string) =>
				workspace_id === "workspace-a"
					? Effect.succeed({
							reader: {
								ReadRegularFile: (path: string, maximum_bytes: number) =>
									Effect.sync(() => {
										reads.push({ maximum_bytes, path });

										return content;
									}),
							},
							workspace_id,
						})
					: Effect.fail({
							_tag: "WorkspaceBoundedRegularFileStoreNotFoundError",
							workspace_id,
						}),
			ListWorkspaceIds: Effect.succeed(["workspace-a"]),
		} as typeof WorkspaceBoundedRegularFileStoreRegistry.Service);
		const runtime = make_backend_runtime({
			database_path: join(directory, "artisan.db"),
			migrations_path,
			workspace_bounded_regular_file_store_registry: workspace_registry,
		});

		try {
			const registry = await runtime.runPromise(ToolRegistry);
			const context = {
				agent_id: "agent",
				run_id: "run",
				thread_id: "thread",
				workspace_id: "workspace-a",
			};
			const listed = await runtime.runPromise(registry.List(context));
			const result = await runtime.runPromise(
				registry.Invoke(
					tool_invocation(context, { revision: 1, tool_id: "workspace.file.read" }),
					{
						path: "src/example.ts",
					},
				),
			);

			expect(listed.tools).toEqual(
				expect.arrayContaining([
					expect.objectContaining({
						descriptor: expect.objectContaining({ tool_id: "workspace.file.read" }),
						state: "eligible",
					}),
				]),
			);
			expect(result).toEqual({
				content: "production",
				identity: {
					algorithm: "sha256",
					byte_count: content.byteLength,
					content_hash: createHash("sha256").update(content).digest("hex"),
				},
				path: "src/example.ts",
				workspace_id: "workspace-a",
			});
			expect(reads).toEqual([
				{ maximum_bytes: workspace_text_maximum_bytes, path: "src/example.ts" },
			]);
		} finally {
			await runtime.dispose();
		}
	});
});
