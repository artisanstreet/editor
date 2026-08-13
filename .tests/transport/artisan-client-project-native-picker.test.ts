import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Layer } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { make_backend_runtime, NativeDirectoryPicker, ProtocolServer } from "@artisan/backend";
import { make_transport_test_harness_with_protocol_server } from "./message-channel-harness";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];

const MakeDirectory = async (label: string) => {
	const directory = await mkdtemp(join(tmpdir(), `artisan-${label}-`));
	temporary_directories.push(directory);
	return directory;
};

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("ArtisanClient native project picker", () => {
	it("carries only Forge's opaque selection over transport, then uses the existing attach flow", async () => {
		const state_directory = await MakeDirectory("native-picker-state");
		const selected_directory = await MakeDirectory("native-picker-selected");
		const runtime = make_backend_runtime({
			database_path: join(state_directory, "artisan.db"),
			migrations_path,
			native_directory_picker: Layer.succeed(NativeDirectoryPicker, {
				Pick: () => Effect.succeed({ kind: "selected", path: selected_directory }),
			}),
			project_directory_roots: [state_directory],
		});
		const protocol_server = await runtime.runPromise(ProtocolServer);
		const harness = await make_transport_test_harness_with_protocol_server(protocol_server);

		try {
			const picked = await Effect.runPromise(harness.client.PickProjectDirectory);
			expect(picked.status).toBe("selected");
			expect(JSON.stringify(picked)).not.toContain(selected_directory);

			if (picked.status !== "selected") throw new Error("expected selected directory");
			const project = await Effect.runPromise(
				harness.client.SelectProjectDirectory({
					directory_id: picked.directory.directory_id,
				}),
			);
			expect(project.display_name).toBe(selected_directory.split(/[\\/]/u).at(-1));
			expect((await Effect.runPromise(harness.client.ListProjects)).projects).toEqual([
				project,
			]);
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	});

	it("round-trips cancellation without mutating the project catalog", async () => {
		const state_directory = await MakeDirectory("native-picker-cancelled");
		const runtime = make_backend_runtime({
			database_path: join(state_directory, "artisan.db"),
			migrations_path,
			native_directory_picker: Layer.succeed(NativeDirectoryPicker, {
				Pick: () => Effect.succeed({ kind: "cancelled" }),
			}),
			project_directory_roots: [state_directory],
		});
		const protocol_server = await runtime.runPromise(ProtocolServer);
		const harness = await make_transport_test_harness_with_protocol_server(protocol_server);

		try {
			await expect(Effect.runPromise(harness.client.PickProjectDirectory)).resolves.toEqual({
				status: "cancelled",
			});
			await expect(Effect.runPromise(harness.client.ListProjects)).resolves.toEqual({
				projects: [],
			});
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	});
});
