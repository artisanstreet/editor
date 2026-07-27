import {
	SelectReleaseArtifact,
	type SemanticVersion,
	type TargetPlatform,
} from "@artisan/distribution";
import { Result, Schema } from "effect";
import { describe, expect, it } from "vitest";
import {
	ReleaseManifest as ReleaseManifestSchema,
	SemanticVersion as SemanticVersionSchema,
} from "@artisan/distribution";

const version = (value: string) => Schema.decodeUnknownSync(SemanticVersionSchema)(value);

const release = Schema.decodeUnknownSync(ReleaseManifestSchema)({
	format_version: 1,
	product_version: "1.2.0",
	editor_forge_compatibility_version: "1.2.0",
	channel: "stable",
	signing_identity: { key_id: "release", algorithm: "ed25519" },
	minimum_bootstrap_version: "0.4.0",
	minimum_cli_version: "1.0.0",
	artifacts: [
		{
			artifact_id: "windows-x64",
			platform: "windows",
			architecture: "x64",
			archive_format: "zip",
			file_name: "artisan-windows-x64.zip",
			byte_size: 100,
			sha256: "a".repeat(64),
			archive_entries: ["bin/ae.exe"],
		},
		{
			artifact_id: "linux-x64-glibc",
			platform: "linux",
			architecture: "x64",
			libc: "glibc",
			archive_format: "tar.zst",
			file_name: "artisan-linux-x64-glibc.tar.zst",
			byte_size: 100,
			sha256: "b".repeat(64),
			archive_entries: ["bin/ae"],
		},
	],
});

const select = (
	target: TargetPlatform,
	bootstrap_version: SemanticVersion = version("0.4.0"),
	cli_version?: SemanticVersion,
) =>
	SelectReleaseArtifact({
		release,
		target,
		bootstrap_version,
		...(cli_version === undefined ? {} : { cli_version }),
	});

describe("SelectReleaseArtifact", () => {
	it("selects the exact platform, architecture, and libc artifact", () => {
		const result = select({ platform: "linux", architecture: "x64", libc: "glibc" });
		expect(Result.isSuccess(result)).toBe(true);
		if (Result.isSuccess(result)) expect(result.success.artifact_id).toBe("linux-x64-glibc");
	});

	it("fails before download for unsupported targets", () => {
		const result = select({ platform: "macos", architecture: "arm64" });
		expect(result).toMatchObject({ _tag: "Failure", failure: { _tag: "UnsupportedTarget" } });
	});

	it("enforces bootstrap and permanent CLI compatibility", () => {
		expect(
			select({ platform: "windows", architecture: "x64" }, version("0.3.9")),
		).toMatchObject({
			failure: { _tag: "BootstrapVersionUnsupported" },
		});
		expect(
			select(
				{ platform: "windows", architecture: "x64" },
				version("0.4.0"),
				version("0.9.9"),
			),
		).toMatchObject({ failure: { _tag: "CliVersionUnsupported" } });
	});
});
