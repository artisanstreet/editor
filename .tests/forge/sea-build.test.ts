import { createHash } from "node:crypto";

import { describe, expect, it } from "vitest";

import { GenerateSeaBuildArtifacts } from "../../.scripts/build/sea/config";

const Sha256 = (value: string) => createHash("sha256").update(value).digest("hex");

describe("SEA build artifacts", () => {
	it("sorts SEA keys and emits reproducible config and manifest JSON", () => {
		const bytes = new Map([
			["z-source", Buffer.from("z")],
			["a-source", Buffer.from("a")],
		]);
		const first = GenerateSeaBuildArtifacts(
			{
				assets: [
					{
						asset_id: "node-pty.win32-x64",
						relative_path: "node-pty/prebuilds/win32-x64/pty.node",
						source_path: "z-source",
					},
					{
						asset_id: "artisan-process-host.win32-x64",
						executable: true,
						relative_path: "artisan-runtime/process-host.exe",
						source_path: "a-source",
					},
				],
				main_path: "C:/forge/host.mjs",
				output_path: "C:/forge/sea.blob",
			},
			(source_path) => bytes.get(source_path)!,
		);

		expect(first.asset_manifest.assets.map((asset) => asset.asset_id)).toEqual([
			"artisan-process-host.win32-x64",
			"node-pty.win32-x64",
		]);
		expect(first.asset_manifest.assets[0]).toMatchObject({
			byte_length: 1,
			executable: true,
			sha256: Sha256("a"),
		});
		expect(first.sea_config.assets).toEqual({
			"artisan-process-host.win32-x64": "a-source",
			"node-pty.win32-x64": "z-source",
		});
		expect(first.sea_config_json).toContain('"useCodeCache": false');
		expect(first.asset_manifest_json).toContain('"version": 1');
	});

	it("rejects unsafe asset identifiers and runtime paths before config generation", () => {
		expect(() =>
			GenerateSeaBuildArtifacts(
				{
					assets: [
						{
							asset_id: "../native",
							relative_path: "native/addon.node",
							source_path: "ignored",
						},
					],
					main_path: "main.mjs",
					output_path: "sea.blob",
				},
				() => Buffer.alloc(0),
			),
		).toThrow("SEA asset id");
		expect(() =>
			GenerateSeaBuildArtifacts(
				{
					assets: [
						{
							asset_id: "native",
							relative_path: "../outside.node",
							source_path: "ignored",
						},
					],
					main_path: "main.mjs",
					output_path: "sea.blob",
				},
				() => Buffer.alloc(0),
			),
		).toThrow("safe relative path");
	});

	it("preserves a scoped package layout while keeping paths relative", () => {
		const artifacts = GenerateSeaBuildArtifacts(
			{
				assets: [
					{
						asset_id: "koffi-native",
						relative_path:
							"native-runtime/node_modules/@koromix/koffi-win32-x64/package.json",
						source_path: "package",
					},
				],
				main_path: "main.cjs",
				output_path: "sea.blob",
			},
			() => Buffer.from("{}"),
		);

		expect(artifacts.asset_manifest.assets[0]?.relative_path).toBe(
			"native-runtime/node_modules/@koromix/koffi-win32-x64/package.json",
		);
	});
});
