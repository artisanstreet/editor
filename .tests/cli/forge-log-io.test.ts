import { appendFile, mkdtemp, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, FileSystem, Stream } from "effect";
import { describe, expect, it } from "vitest";

import { FollowForgeLogFile, MAX_LOG_FOLLOW_READ_BYTES } from "../../modules/cli/src/operations";
import {
	ForgeChildEnvironment,
	MAX_DETACHED_FORGE_LOG_BYTES,
	PrepareDetachedForgeLog,
} from "../../modules/cli/src/node-launcher";

describe("Forge log I/O", () => {
	it("caps a prior detached log before launch", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-forge-log-"));
		const path = join(root, "forge.log");
		try {
			await writeFile(path, "x".repeat(MAX_DETACHED_FORGE_LOG_BYTES));
			await Effect.runPromise(PrepareDetachedForgeLog(path));
			expect((await stat(path)).size).toBe(0);
		} finally {
			await rm(root, { force: true, recursive: true });
		}
	});

	it("reads follow output in bounded ranges", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-forge-log-"));
		const path = join(root, "forge.log");
		try {
			await writeFile(path, "previous output\n");
			const followed = Effect.gen(function* () {
				const file_system = yield* FileSystem.FileSystem;
				return yield* FollowForgeLogFile(path)(file_system).pipe(
					Stream.take(2),
					Stream.runCollect,
				);
			}).pipe(Effect.provide(NodeFileSystem.layer));
			setTimeout(() => {
				void appendFile(path, "a".repeat(MAX_LOG_FOLLOW_READ_BYTES * 2));
			}, 50);
			const chunks = await Effect.runPromise(followed);
			expect(chunks.map((chunk) => chunk.length)).toEqual([
				MAX_LOG_FOLLOW_READ_BYTES,
				MAX_LOG_FOLLOW_READ_BYTES,
			]);
		} finally {
			await rm(root, { force: true, recursive: true });
		}
	});

	it("resumes from the beginning after a visible log truncation", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-forge-log-"));
		const path = join(root, "forge.log");
		try {
			await writeFile(path, "old output ".repeat(10_000));
			const followed = Effect.gen(function* () {
				const file_system = yield* FileSystem.FileSystem;
				return yield* FollowForgeLogFile(path)(file_system).pipe(
					Stream.take(1),
					Stream.runCollect,
				);
			}).pipe(Effect.provide(NodeFileSystem.layer));
			setTimeout(() => {
				void writeFile(path, "replacement output\n");
			}, 50);
			expect(await Effect.runPromise(followed)).toEqual(["replacement output\n"]);
		} finally {
			await rm(root, { force: true, recursive: true });
		}
	});

	it("keeps Codex SQLite and Rust diagnostics Forge-owned", () => {
		const environment = ForgeChildEnvironment(
			{
				config: {
					data_root: "C:/artisan/data",
					listen_host: "127.0.0.1",
					listen_port: 4848,
				},
				instance_id: "forge_1",
				profile: "default",
				token: "secret",
			},
			{
				executable_path: "C:/artisan/Artisan Forge.exe",
				host_entry_path: "C:/artisan/host.js",
				migrations_path: "C:/artisan/migrations",
				native_runtime_path: "C:/artisan/native-runtime",
				node_executable_path: "C:/artisan/node.exe",
				static_frontend_root: "C:/artisan/frontend",
				windows_process_host_path: "C:/artisan/windows-process-host.js",
			},
			{ inherited: { CODEX_HOME: "C:/user/codex", RUST_LOG: "trace" }, node_path: undefined },
			{
				log_path: "C:/artisan/forge.log",
				state_path: "C:/artisan/state.json",
			},
		);
		expect(environment).toMatchObject({
			CODEX_HOME: "C:/user/codex",
			CODEX_SQLITE_HOME: join("C:/artisan/data", "codex-sqlite"),
			RUST_LOG: "warn",
			ARTISAN_FORGE_LOG_PATH: "C:/artisan/forge.log",
		});
	});
});
