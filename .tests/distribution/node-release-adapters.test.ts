import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import { describe, expect, it } from "vitest";
import { Effect, Layer } from "effect";
import { ZipFile } from "yazl";

import {
	ArtifactStager,
	ForgeUpdateLifecycle,
	InstallationHealth,
	ReleaseSource,
} from "../../modules/distribution/src/installer";
import {
	CanonicalizeWindowsArchivePath,
	HealthProcessHost,
	make_github_release_source_layer,
	make_node_forge_update_lifecycle_layer,
	make_node_installation_health_layer,
	NodeZipArtifactStagerLive,
	SecureReleaseHttp,
	type HealthProcessRequest,
	type HttpResult,
} from "../../modules/distribution/src/node-release-adapters";
import type {
	ReleaseArtifact,
	ReleaseManifest,
} from "../../modules/distribution/src/release-manifest";

const EncodeZip = (
	entries: ReadonlyArray<{
		readonly contents: string;
		readonly mode?: number;
		readonly path: string;
	}>,
) =>
	new Promise<Uint8Array>((resolve, reject) => {
		const zip = new ZipFile();
		const chunks: Array<Buffer> = [];
		zip.outputStream.on("data", (chunk: Buffer) => chunks.push(chunk));
		zip.outputStream.on("error", reject);
		zip.outputStream.on("end", () => resolve(Buffer.concat(chunks)));
		for (const entry of entries) {
			if (entry.mode === undefined) zip.addBuffer(Buffer.from(entry.contents), entry.path);
			else
				zip.addBuffer(Buffer.from(entry.contents), entry.path, {
					mode: entry.mode,
				});
		}
		zip.end();
	});

const MakeArtifact = (byte_size: number, entries: ReadonlyArray<string>): ReleaseArtifact => ({
	artifact_id: "artisan-windows-x64-0.2.0",
	platform: "windows",
	architecture: "x64",
	archive_format: "zip",
	file_name: "artisan-windows-x64.zip",
	byte_size,
	sha256: "a".repeat(64),
	archive_entries: entries as [string, ...Array<string>],
});

const MakeRelease = (artifact: ReleaseArtifact): ReleaseManifest => ({
	format_version: 1,
	product_version: "0.2.0",
	editor_forge_compatibility_version: "0.2.0",
	channel: "stable",
	signing_identity: { algorithm: "ed25519", key_id: "test-key" },
	minimum_bootstrap_version: "0.1.0",
	minimum_cli_version: "0.1.0",
	artifacts: [artifact],
});

describe("Node release adapters", () => {
	it("rejects Windows device, ADS, alias, traversal, and non-canonical archive paths", async () => {
		for (const path of [
			"CON",
			"con.txt",
			"devices/COM1.log",
			"bin/ae.exe.",
			"bin/ae.exe ",
			"bin/ae.exe:payload",
			"bin\\ae.exe",
			"bin//ae.exe",
			"bin/./ae.exe",
			"bin/../ae.exe",
			"/absolute/ae.exe",
			"C:/absolute/ae.exe",
			"bin/\u00e6.exe",
		]) {
			await expect(
				Effect.runPromise(CanonicalizeWindowsArchivePath(path)),
				`expected ${JSON.stringify(path)} to be rejected`,
			).rejects.toBeDefined();
		}
		await expect(
			Effect.runPromise(CanonicalizeWindowsArchivePath("forge/native-runtime/node.exe")),
		).resolves.toEqual({
			canonical: "forge/native-runtime/node.exe",
			path: "forge/native-runtime/node.exe",
			segments: ["forge", "native-runtime", "node.exe"],
		});
	});

	it("resolves both signed manifest files through the injected bounded transport", async () => {
		const requested: Array<{ readonly maximum_bytes: number; readonly url: string }> = [];
		const manifest = new TextEncoder().encode('{"format_version":1}');
		const signature = new TextEncoder().encode(
			'{"algorithm":"ed25519","key_id":"test-key","signature":"value"}',
		);
		const http = Layer.succeed(
			SecureReleaseHttp,
			SecureReleaseHttp.of({
				Get: (url, maximum_bytes) =>
					Effect.sync(() => {
						requested.push({ maximum_bytes, url });
						return {
							status: 200,
							url,
							bytes: url.endsWith(".sig") ? signature : manifest,
						} satisfies HttpResult;
					}),
			}),
		);
		const layer = make_github_release_source_layer({
			owner: "sandersonstabo",
			repository: "artisan-editor",
		}).pipe(Layer.provide(http));

		const envelope = await Effect.runPromise(
			Effect.gen(function* () {
				return yield* (yield* ReleaseSource).Resolve("stable");
			}).pipe(Effect.provide(layer)),
		);

		expect(envelope.manifest).toEqual(manifest);
		expect(envelope.signature).toMatchObject({ key_id: "test-key" });
		expect(requested.map(({ url }) => url)).toEqual([
			"https://github.com/sandersonstabo/artisan-editor/releases/latest/download/release-manifest.json",
			"https://github.com/sandersonstabo/artisan-editor/releases/latest/download/release-manifest.sig",
		]);
		expect(requested.every(({ maximum_bytes }) => maximum_bytes === 1024 * 1024)).toBe(true);
	});

	it("extracts exactly the declared regular files and atomically publishes the version", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-node-stager-"));
		try {
			const bytes = await EncodeZip([
				{ contents: "cli", path: "bin/ae.exe" },
				{ contents: "forge", path: "Artisan Forge.exe" },
			]);
			const artifact = MakeArtifact(bytes.byteLength, ["bin/ae.exe", "Artisan Forge.exe"]);
			const staging_path = join(root, "staging", "0.2.0");
			const version_path = join(root, "versions", "0.2.0");
			const layer = NodeZipArtifactStagerLive.pipe(
				Layer.provide(Layer.mergeAll(NodeFileSystem.layer, NodePath.layer)),
			);

			await Effect.runPromise(
				Effect.gen(function* () {
					yield* (yield* ArtifactStager).Stage({
						artifact,
						bytes,
						release: MakeRelease(artifact),
						staging_path,
						version_path,
					});
				}).pipe(Effect.provide(layer)),
			);

			expect(await readFile(join(version_path, "bin", "ae.exe"), "utf8")).toBe("cli");
			expect(await readFile(join(version_path, "Artisan Forge.exe"), "utf8")).toBe("forge");

			await expect(
				Effect.runPromise(
					Effect.gen(function* () {
						yield* (yield* ArtifactStager).Stage({
							artifact,
							bytes,
							release: MakeRelease(artifact),
							staging_path,
							version_path,
						});
					}).pipe(Effect.provide(layer)),
				),
			).resolves.toBeUndefined();

			await writeFile(join(version_path, "bin", "ae.exe"), "tampered");
			await expect(
				Effect.runPromise(
					Effect.gen(function* () {
						yield* (yield* ArtifactStager).Stage({
							artifact,
							bytes,
							release: MakeRelease(artifact),
							staging_path,
							version_path,
						});
					}).pipe(Effect.provide(layer)),
				),
			).rejects.toMatchObject({ _tag: "ArtifactStagingFailure" });
		} finally {
			await rm(root, { force: true, recursive: true });
		}
	});

	it("rejects undeclared and duplicate ZIP entries without publishing", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-node-stager-reject-"));
		try {
			const layer = NodeZipArtifactStagerLive.pipe(
				Layer.provide(Layer.mergeAll(NodeFileSystem.layer, NodePath.layer)),
			);
			for (const entries of [
				[
					{ contents: "cli", path: "ae.exe" },
					{ contents: "surprise", path: "extra.exe" },
				],
				[
					{ contents: "first", path: "ae.exe" },
					{ contents: "second", path: "ae.exe" },
				],
				[{ contents: "target.exe", mode: 0o120777, path: "ae.exe" }],
			]) {
				const bytes = await EncodeZip(entries);
				const artifact = MakeArtifact(bytes.byteLength, ["ae.exe"]);
				const version_path = join(root, "versions", String(Math.random()));
				await expect(
					Effect.runPromise(
						Effect.gen(function* () {
							yield* (yield* ArtifactStager).Stage({
								artifact,
								bytes,
								release: MakeRelease(artifact),
								staging_path: join(root, "staging", String(Math.random())),
								version_path,
							});
						}).pipe(Effect.provide(layer)),
					),
				).rejects.toMatchObject({ _tag: "ArtifactStagingFailure" });
			}
		} finally {
			await rm(root, { force: true, recursive: true });
		}
	});

	it("rejects case-insensitive ZIP and manifest aliases before creating staging", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-node-stager-alias-"));
		try {
			const layer = NodeZipArtifactStagerLive.pipe(
				Layer.provide(Layer.mergeAll(NodeFileSystem.layer, NodePath.layer)),
			);
			for (const declared of [
				["Bin/ae.exe", "bin/AE.exe"],
				["bin/ae.exe:payload"],
				["NUL.txt"],
			]) {
				const entries = declared.map((path) => ({ contents: path, path }));
				const bytes = await EncodeZip(entries);
				const artifact = MakeArtifact(bytes.byteLength, declared);
				const staging_path = join(root, "staging", String(Math.random()));
				await expect(
					Effect.runPromise(
						Effect.gen(function* () {
							yield* (yield* ArtifactStager).Stage({
								artifact,
								bytes,
								release: MakeRelease(artifact),
								staging_path,
								version_path: join(root, "versions", String(Math.random())),
							});
						}).pipe(Effect.provide(layer)),
					),
				).rejects.toMatchObject({ _tag: "ArtifactStagingFailure" });
				await expect(
					import("node:fs/promises").then(({ stat }) => stat(staging_path)),
				).rejects.toMatchObject({ code: "ENOENT" });
			}
		} finally {
			await rm(root, { force: true, recursive: true });
		}
	});

	it("validates staged runtime files and runs ae --version through an absolute process request", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-health-adapter-"));
		const launcher = join(root, "versions", "0.2.0", "bin", "ae.cmd");
		const runtime_files = [
			launcher,
			join(root, "versions", "0.2.0", "forge", "ae.js"),
			join(root, "versions", "0.2.0", "forge", "Artisan Forge.exe"),
			join(root, "versions", "0.2.0", "editor", "Artisan Editor.exe"),
		];
		for (const path of runtime_files) {
			await mkdir(join(path, ".."), { recursive: true });
			await writeFile(path, "fixture");
		}
		const requests: Array<HealthProcessRequest> = [];
		const host = Layer.succeed(
			HealthProcessHost,
			HealthProcessHost.of({
				Run: (request) =>
					Effect.sync(() => {
						requests.push(request);
						return { exit_code: 0, stderr: "", stdout: "ae v0.2.0\n" };
					}),
			}),
		);
		const command_interpreter = "C:\\Windows\\System32\\cmd.exe";
		const layer = make_node_installation_health_layer(root, command_interpreter).pipe(
			Layer.provide(Layer.mergeAll(host, NodeFileSystem.layer, NodePath.layer)),
		);

		try {
			await Effect.runPromise(
				Effect.gen(function* () {
					yield* (yield* InstallationHealth).Check("0.2.0");
				}).pipe(Effect.provide(layer)),
			);

			expect(requests).toEqual([
				{
					argv: ["/d", "/s", "/c", `""${launcher}" --version"`],
					cwd: join(root, "versions", "0.2.0"),
					executable: command_interpreter,
					timeout_ms: 30_000,
					windows_verbatim_arguments: true,
				},
			]);
		} finally {
			await rm(root, { force: true, recursive: true });
		}
	});

	it("maps non-zero health commands to a typed installation failure", async () => {
		const host = Layer.succeed(
			HealthProcessHost,
			HealthProcessHost.of({
				Run: () => Effect.succeed({ exit_code: 1, stderr: "invalid", stdout: "" }),
			}),
		);
		const layer = make_node_installation_health_layer(
			"C:\\Artisan",
			"C:\\Windows\\System32\\cmd.exe",
		).pipe(Layer.provide(Layer.mergeAll(host, NodeFileSystem.layer, NodePath.layer)));

		await expect(
			Effect.runPromise(
				Effect.gen(function* () {
					yield* (yield* InstallationHealth).Check("0.2.0");
				}).pipe(Effect.provide(layer)),
			),
		).rejects.toMatchObject({
			_tag: "InstallationHealthFailure",
			version: "0.2.0",
		});
	});

	it("stops every running owned profile and restarts exactly that set on the new version", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-forge-update-"));
		await mkdir(join(root, "profiles", "default"), { recursive: true });
		await writeFile(join(root, "profiles", "default", "config.json"), "{}");
		const outputs = [
			'{"state":"running"}',
			"",
			'{"state":"missing"}',
			"",
			'{"state":"running"}',
			'{"distribution":{"healthy":true},"forge":{"healthy":true}}',
		];
		const requests: Array<HealthProcessRequest> = [];
		const host = Layer.succeed(
			HealthProcessHost,
			HealthProcessHost.of({
				Run: (request) =>
					Effect.sync(() => {
						requests.push(request);
						return {
							exit_code: 0,
							stderr: "",
							stdout: outputs.shift() ?? "",
						};
					}),
			}),
		);
		const layer = make_node_forge_update_lifecycle_layer(root, "cmd.exe").pipe(
			Layer.provide(Layer.mergeAll(host, NodeFileSystem.layer, NodePath.layer)),
		);
		try {
			const lifecycle = await Effect.runPromise(
				ForgeUpdateLifecycle.pipe(Effect.provide(layer)),
			);
			const snapshot = await Effect.runPromise(lifecycle.Quiesce("0.1.0"));
			expect(snapshot).toEqual({ running_profiles: ["default"] });
			await Effect.runPromise(lifecycle.ResumeAndVerify("0.2.0", snapshot));
			expect(requests.map((request) => request.argv.at(-1))).toEqual([
				expect.stringContaining(
					'versions\\0.1.0\\bin\\ae.cmd" status --profile default --json',
				),
				expect.stringContaining('versions\\0.1.0\\bin\\ae.cmd" stop --profile default'),
				expect.stringContaining(
					'versions\\0.1.0\\bin\\ae.cmd" status --profile default --json',
				),
				expect.stringContaining('versions\\0.2.0\\bin\\ae.cmd" start --profile default'),
				expect.stringContaining(
					'versions\\0.2.0\\bin\\ae.cmd" status --profile default --json',
				),
				expect.stringContaining(
					'versions\\0.2.0\\bin\\ae.cmd" doctor --profile default --json',
				),
			]);
		} finally {
			await rm(root, { force: true, recursive: true });
		}
	});

	it("treats an installation with no Forge profiles as a valid empty update snapshot", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-forge-update-"));
		let process_calls = 0;
		const host = Layer.succeed(
			HealthProcessHost,
			HealthProcessHost.of({
				Run: () =>
					Effect.sync(() => {
						process_calls += 1;
						return { exit_code: 0, stderr: "", stdout: "" };
					}),
			}),
		);
		const layer = make_node_forge_update_lifecycle_layer(root, "cmd.exe").pipe(
			Layer.provide(Layer.mergeAll(host, NodeFileSystem.layer, NodePath.layer)),
		);
		try {
			const lifecycle = await Effect.runPromise(
				ForgeUpdateLifecycle.pipe(Effect.provide(layer)),
			);
			await expect(Effect.runPromise(lifecycle.Quiesce("0.1.0"))).resolves.toEqual({
				running_profiles: [],
			});
			await expect(
				Effect.runPromise(lifecycle.VerifyCurrent("0.1.0")),
			).resolves.toBeUndefined();
			expect(process_calls).toBe(0);
		} finally {
			await rm(root, { force: true, recursive: true });
		}
	});

	it("rejects non-JSON Forge status as a typed quiesce failure", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-forge-update-"));
		await mkdir(join(root, "profiles", "default"), { recursive: true });
		await writeFile(join(root, "profiles", "default", "config.json"), "{}");
		const host = Layer.succeed(
			HealthProcessHost,
			HealthProcessHost.of({
				Run: () => Effect.succeed({ exit_code: 0, stderr: "", stdout: "[object Event]" }),
			}),
		);
		const layer = make_node_forge_update_lifecycle_layer(root, "cmd.exe").pipe(
			Layer.provide(Layer.mergeAll(host, NodeFileSystem.layer, NodePath.layer)),
		);
		try {
			const lifecycle = await Effect.runPromise(
				ForgeUpdateLifecycle.pipe(Effect.provide(layer)),
			);
			await expect(Effect.runPromise(lifecycle.Quiesce("0.1.0"))).rejects.toMatchObject({
				_tag: "ForgeUpdateLifecycleFailure",
				operation: "quiesce",
				version: "0.1.0",
			});
		} finally {
			await rm(root, { force: true, recursive: true });
		}
	});
});
