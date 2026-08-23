import { Buffer } from "node:buffer";
import { createHash } from "node:crypto";
import { gzipSync } from "node:zlib";

import { describe, expect, it } from "vitest";
import { Effect, Stream } from "effect";

import {
	engine_distributions,
	ExtractNpmTarballExecutable,
	opencode2_certified_version,
	ToolchainReleaseHttp,
	VerifyNpmSha512Integrity,
} from "@artisan/engines";

const header = (name: string, size: number, type = "0") => {
	const result = Buffer.alloc(512);
	result.write(name, 0, 100, "utf8");
	result.write("0000755\0", 100, 8, "ascii");
	result.write("0000000\0", 108, 8, "ascii");
	result.write("0000000\0", 116, 8, "ascii");
	result.write(`${size.toString(8).padStart(11, "0")}\0`, 124, 12, "ascii");
	result.write("00000000000\0", 136, 12, "ascii");
	result.fill(32, 148, 156);
	result.write(type, 156, 1, "ascii");
	result.write("ustar\0", 257, 6, "ascii");
	const checksum = result.reduce((sum, byte) => sum + byte, 0);
	result.write(`${checksum.toString(8).padStart(6, "0")}\0 `, 148, 8, "ascii");
	return result;
};

const tar = (
	entries: ReadonlyArray<{
		readonly body: Buffer;
		readonly name: string;
		readonly type?: string;
	}>,
) =>
	gzipSync(
		Buffer.concat([
			...entries.flatMap((entry) => [
				header(entry.name, entry.body.length, entry.type),
				entry.body,
				Buffer.alloc((512 - (entry.body.length % 512)) % 512),
			]),
			Buffer.alloc(1024),
		]),
	);

describe("safe NPM platform tarball extraction", () => {
	it("pins the separate V2 engine and never registers an OpenCode 1 distribution", async () => {
		expect(
			engine_distributions.some((distribution) => distribution.engine_id === "opencode"),
		).toBe(false);
		const distribution = engine_distributions.find(
			(candidate) => candidate.engine_id === "opencode2",
		);
		expect(distribution?.recommended_version).toBe(opencode2_certified_version);
		const release = await Effect.runPromise(
			distribution!
				.ResolveRelease(opencode2_certified_version, {
					architecture: "x64",
					platform: "win32",
				})
				.pipe(
					Effect.provideService(
						ToolchainReleaseHttp,
						ToolchainReleaseHttp.of({
							Get: () =>
								Effect.die("release metadata must not be fetched for the pin"),
							GetStream: () => Stream.die("release bytes are not used by resolution"),
						}),
					),
				),
		);
		expect(release).toMatchObject({
			archive_member: "package/bin/opencode2.exe",
			artifact_kind: "npm-tarball",
			binary: "opencode2.exe",
		});
	});

	it("verifies SHA-512 and returns only the expected regular executable", () => {
		const archive = tar([
			{ body: Buffer.from("metadata"), name: "package/package.json" },
			{ body: Buffer.from("executable"), name: "package/bin/opencode2.exe" },
		]);
		const integrity = createHash("sha512").update(archive).digest("base64");
		expect(() => VerifyNpmSha512Integrity(archive, integrity)).not.toThrow();
		expect(
			Buffer.from(
				ExtractNpmTarballExecutable(archive, "package/bin/opencode2.exe", 1024),
			).toString(),
		).toBe("executable");
	});

	it("rejects traversal, links, duplicates, and integrity drift", () => {
		const executable = { body: Buffer.from("binary"), name: "package/bin/opencode2.exe" };
		expect(() =>
			ExtractNpmTarballExecutable(
				tar([{ body: Buffer.from("bad"), name: "../escape" }, executable]),
				executable.name,
				1024,
			),
		).toThrow(/Unsafe tar member path/);
		expect(() =>
			ExtractNpmTarballExecutable(tar([{ ...executable, type: "2" }]), executable.name, 1024),
		).toThrow(/Unsupported tar member type/);
		expect(() =>
			ExtractNpmTarballExecutable(tar([executable, executable]), executable.name, 1024),
		).toThrow(/Duplicate tar member/);
		expect(() =>
			VerifyNpmSha512Integrity(tar([executable]), Buffer.alloc(64).toString("base64")),
		).toThrow(/integrity mismatch/);
	});
});
