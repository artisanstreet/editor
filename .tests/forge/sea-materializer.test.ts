import { createHash } from "node:crypto";
import { mkdtemp, readFile, readdir, rm } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";

import { NodeCrypto, NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import { Effect, Layer } from "effect";
import { describe, expect, it } from "vitest";

import { SeaAssetSource } from "../../modules/forge/src/sea/asset-source";
import {
	make_sea_asset_materializer_layer,
	SeaAssetMaterializer,
} from "../../modules/forge/src/sea/materializer";

const Sha256 = (value: string) => createHash("sha256").update(value).digest("hex");

const make_source = (assets: ReadonlyMap<string, Uint8Array>, reads: Array<string>) =>
	Layer.succeed(
		SeaAssetSource,
		SeaAssetSource.of({
			Read: (asset_id) =>
				Effect.gen(function* () {
					reads.push(asset_id);
					const bytes = assets.get(asset_id);
					return bytes ?? new Uint8Array();
				}),
		}),
	);

describe("SEA asset materializer", () => {
	it("stages one verified package layout under a manifest content root and reuses it", async () => {
		const cache_root = await mkdtemp(join(tmpdir(), "artisan-sea-cache-"));
		const reads: Array<string> = [];
		const node_pty = Buffer.from("node-pty-native");
		const package_json = Buffer.from('{"name":"node-pty"}');
		const source = make_source(
			new Map([
				["node-pty.binding", node_pty],
				["node-pty.package", package_json],
			]),
			reads,
		);
		const dependencies = Layer.mergeAll(
			source,
			NodeCrypto.layer,
			NodeFileSystem.layer,
			NodePath.layer,
		);
		const layer = make_sea_asset_materializer_layer({ cache_root }).pipe(
			Layer.provide(dependencies),
		);
		const manifest = {
			assets: [
				{
					asset_id: "node-pty.binding",
					byte_length: node_pty.byteLength,
					executable: false,
					relative_path: "node-pty/prebuilds/win32-x64/pty.node",
					sha256: Sha256("node-pty-native"),
				},
				{
					asset_id: "node-pty.package",
					byte_length: package_json.byteLength,
					executable: false,
					relative_path: "node-pty/package.json",
					sha256: Sha256('{"name":"node-pty"}'),
				},
			],
			version: 1,
		};
		const program = Effect.gen(function* () {
			const materializer = yield* SeaAssetMaterializer;
			const first = yield* materializer.Materialize(manifest);
			const second = yield* materializer.Materialize(manifest);
			return { first, second };
		});

		try {
			const { first, second } = await Effect.runPromise(program.pipe(Effect.provide(layer)));
			expect(first.root).toBe(second.root);
			expect(first.assets.map((asset) => asset.path)).toEqual([
				join(first.root, "node-pty", "prebuilds", "win32-x64", "pty.node"),
				join(first.root, "node-pty", "package.json"),
			]);
			expect(await readFile(first.assets[0]!.path, "utf8")).toBe("node-pty-native");
			expect(reads).toEqual(["node-pty.binding", "node-pty.package"]);
		} finally {
			await rm(cache_root, { force: true, recursive: true });
		}
	});

	it("does not publish a partial package when a SEA asset fails integrity verification", async () => {
		const cache_root = await mkdtemp(join(tmpdir(), "artisan-sea-cache-"));
		const reads: Array<string> = [];
		const source = make_source(new Map([["native", Buffer.from("tampered")]]), reads);
		const dependencies = Layer.mergeAll(
			source,
			NodeCrypto.layer,
			NodeFileSystem.layer,
			NodePath.layer,
		);
		const layer = make_sea_asset_materializer_layer({ cache_root }).pipe(
			Layer.provide(dependencies),
		);
		const program = Effect.gen(function* () {
			const materializer = yield* SeaAssetMaterializer;
			return yield* materializer.Materialize({
				assets: [
					{
						asset_id: "native",
						byte_length: 8,
						executable: false,
						relative_path: "native/addon.node",
						sha256: Sha256("expected"),
					},
				],
				version: 1,
			});
		});

		try {
			const exit = await Effect.runPromise(program.pipe(Effect.exit, Effect.provide(layer)));
			expect(exit._tag).toBe("Failure");
			expect(reads).toEqual(["native"]);
			expect(await readdir(cache_root)).toEqual([]);
		} finally {
			await rm(cache_root, { force: true, recursive: true });
		}
	});

	it("schema-rejects traversal paths before reading a SEA asset", async () => {
		const cache_root = await mkdtemp(join(tmpdir(), "artisan-sea-cache-"));
		const reads: Array<string> = [];
		const source = make_source(new Map(), reads);
		const dependencies = Layer.mergeAll(
			source,
			NodeCrypto.layer,
			NodeFileSystem.layer,
			NodePath.layer,
		);
		const layer = make_sea_asset_materializer_layer({ cache_root }).pipe(
			Layer.provide(dependencies),
		);
		const program = Effect.gen(function* () {
			const materializer = yield* SeaAssetMaterializer;
			return yield* materializer.Materialize({
				assets: [
					{
						asset_id: "native",
						byte_length: 0,
						executable: false,
						relative_path: "../outside.node",
						sha256: Sha256(""),
					},
				],
				version: 1,
			});
		});

		try {
			const exit = await Effect.runPromise(program.pipe(Effect.exit, Effect.provide(layer)));
			expect(exit._tag).toBe("Failure");
			expect(reads).toEqual([]);
		} finally {
			await rm(cache_root, { force: true, recursive: true });
		}
	});

	it("accepts the bounded production asset count", async () => {
		const cache_root = await mkdtemp(join(tmpdir(), "artisan-sea-cache-"));
		const reads: Array<string> = [];
		const source = make_source(new Map(), reads);
		const dependencies = Layer.mergeAll(
			source,
			NodeCrypto.layer,
			NodeFileSystem.layer,
			NodePath.layer,
		);
		const layer = make_sea_asset_materializer_layer({ cache_root }).pipe(
			Layer.provide(dependencies),
		);
		const assets = Array.from({ length: 137 }, (_, index) => {
			const suffix = index.toString().padStart(3, "0");

			return {
				asset_id: `asset-${suffix}`,
				byte_length: 0,
				executable: false,
				relative_path: `assets/asset-${suffix}`,
				sha256: Sha256(""),
			};
		});

		try {
			const materialized = await Effect.runPromise(
				Effect.gen(function* () {
					const materializer = yield* SeaAssetMaterializer;
					return yield* materializer.Materialize({ assets, version: 1 });
				}).pipe(Effect.provide(layer)),
			);
			expect(materialized.assets).toHaveLength(137);
			expect(reads).toHaveLength(137);
		} finally {
			await rm(cache_root, { force: true, recursive: true });
		}
	});

	it("accepts one concurrent publisher and reuses its verified root", async () => {
		const cache_root = await mkdtemp(join(tmpdir(), "artisan-sea-cache-"));
		const bytes = Buffer.from("concurrent-native");
		let read_count = 0;
		let release!: () => void;
		const both_reading = new Promise<void>((accept) => (release = accept));
		const source = Layer.succeed(
			SeaAssetSource,
			SeaAssetSource.of({
				Read: () =>
					Effect.promise(async () => {
						read_count += 1;
						if (read_count === 2) release();
						await both_reading;

						return bytes;
					}),
			}),
		);
		const dependencies = Layer.mergeAll(
			source,
			NodeCrypto.layer,
			NodeFileSystem.layer,
			NodePath.layer,
		);
		const layer = make_sea_asset_materializer_layer({ cache_root }).pipe(
			Layer.provide(dependencies),
		);
		const manifest = {
			assets: [
				{
					asset_id: "native",
					byte_length: bytes.byteLength,
					executable: false,
					relative_path: "native/addon.node",
					sha256: Sha256("concurrent-native"),
				},
			],
			version: 1,
		};

		try {
			const [first, second] = await Effect.runPromise(
				Effect.gen(function* () {
					const materializer = yield* SeaAssetMaterializer;
					return yield* Effect.all(
						[materializer.Materialize(manifest), materializer.Materialize(manifest)],
						{ concurrency: "unbounded" },
					);
				}).pipe(Effect.provide(layer)),
			);
			expect(first.root).toBe(second.root);
			expect(read_count).toBe(2);
			expect(await readFile(first.assets[0]!.path, "utf8")).toBe("concurrent-native");
		} finally {
			release();
			await rm(cache_root, { force: true, recursive: true });
		}
	});
});
