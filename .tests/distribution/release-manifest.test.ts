import {
	MaximumArchiveEntries,
	MaximumArtifactBytes,
	ReleaseManifest,
} from "@artisan/distribution";
import { Schema } from "effect";
import { describe, expect, it } from "vitest";

const valid_manifest = {
	format_version: 1,
	product_version: "0.2.0",
	editor_forge_compatibility_version: "0.2.0",
	channel: "stable",
	signing_identity: { key_id: "artisan-release-2026", algorithm: "ed25519" },
	minimum_installer_version: "0.1.0",
	minimum_cli_version: "0.1.0",
	artifacts: [
		{
			artifact_id: "windows-x64",
			platform: "windows",
			architecture: "x64",
			archive_format: "zip",
			file_name: "artisan-windows-x64.zip",
			byte_size: 42,
			sha256: "a".repeat(64),
			archive_entries: ["bin/ae.exe", "editor/Artisan Editor.exe", "forge/Artisan Forge.exe"],
		},
	],
} as const;

describe("ReleaseManifest", () => {
	it("decodes a signed compatibility release contract", () => {
		expect(Schema.decodeUnknownSync(ReleaseManifest)(valid_manifest)).toEqual(valid_manifest);
	});

	it("rejects unsafe archive entries and malformed digests", () => {
		expect(() =>
			Schema.decodeUnknownSync(ReleaseManifest)({
				...valid_manifest,
				artifacts: [
					{
						...valid_manifest.artifacts[0],
						sha256: "nope",
						archive_entries: ["../escape"],
					},
				],
			}),
		).toThrow();
	});

	it("rejects artifacts and central directories above bootstrap resource limits", () => {
		expect(() =>
			Schema.decodeUnknownSync(ReleaseManifest)({
				...valid_manifest,
				artifacts: [
					{
						...valid_manifest.artifacts[0],
						byte_size: MaximumArtifactBytes + 1,
					},
				],
			}),
		).toThrow();
		expect(() =>
			Schema.decodeUnknownSync(ReleaseManifest)({
				...valid_manifest,
				artifacts: [
					{
						...valid_manifest.artifacts[0],
						archive_entries: Array.from(
							{ length: MaximumArchiveEntries + 1 },
							(_, index) => `entry-${index}`,
						),
					},
				],
			}),
		).toThrow();
	});
});
