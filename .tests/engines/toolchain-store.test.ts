import { describe, expect, it } from "vitest";
import { Schema } from "effect";
import { readFileSync } from "node:fs";

import { ToolchainGeneration, make_toolchain_layout } from "@artisan/engines";

/** Stands in for the platform `Path` service so layout assertions stay pure. */
const join_path = {
	join: (...parts: ReadonlyArray<string>) => parts.join("/"),
} as never;

describe("managed toolchain generations", () => {
	it("sets both Claude's owned config home and supported update suppression", () => {
		const service_source = readFileSync(
			new URL("../../modules/engines/src/toolchain/service.ts", import.meta.url),
			"utf8",
		);

		expect(service_source).toContain(
			"[distribution.home_environment_variable]: layout.home_path",
		);
		expect(service_source).toContain('DISABLE_UPDATES: "1"');
		expect(service_source).toContain('DISABLE_INSTALLATION_CHECKS: "1"');
		expect(service_source).not.toContain("autoUpdates: false");
	});

	it("uses an immutable unique generation directory rather than a mutable version directory", () => {
		const layout = make_toolchain_layout("C:/artisan/toolchain", "codex", {
			join: (...parts: ReadonlyArray<string>) => parts.join("/"),
		} as never);
		const generation = {
			binary: "codex.exe",
			directory: "0.145.0-unique-generation",
			sha256: "a".repeat(64),
			version: "0.145.0",
		};

		expect(layout.executable_path(generation)).toBe(
			"C:/artisan/toolchain/codex/versions/0.145.0-unique-generation/codex.exe",
		);
		expect(Schema.decodeUnknownSync(ToolchainGeneration)(generation)).toEqual(generation);
	});

	it("keeps the default profile on the pre-profile home so an existing sign-in survives", () => {
		const layout = make_toolchain_layout("C:/artisan/toolchain", "claude", join_path);

		expect(layout.home_path).toBe("C:/artisan/toolchain/claude/home");
		expect(layout.profile_id).toBe("default");
	});

	it("gives an added profile its own home while sharing the engine's installed binary", () => {
		const generation = {
			binary: "claude.exe",
			directory: "2.1.0-unique-generation",
			sha256: "b".repeat(64),
			version: "2.1.0",
		};
		const work = make_toolchain_layout("C:/artisan/toolchain", "claude", join_path, "work");
		const personal = make_toolchain_layout(
			"C:/artisan/toolchain",
			"claude",
			join_path,
			"personal",
		);

		expect(work.home_path).toBe("C:/artisan/toolchain/claude/homes/work");
		expect(personal.home_path).toBe("C:/artisan/toolchain/claude/homes/personal");
		/** One verified install serves every profile; only the config home differs. */
		expect(work.executable_path(generation)).toBe(personal.executable_path(generation));
		expect(work.state_path).toBe(personal.state_path);
	});

	it("rejects a profile identifier that could escape the engine layout", () => {
		for (const hostile of ["../../etc", "work/../..", ".", "", "a\\b"])
			expect(() =>
				make_toolchain_layout("C:/artisan/toolchain", "claude", join_path, hostile),
			).toThrow(/Unsafe toolchain profile identifier/);
	});

	it("rejects a generation without an integrity digest", () => {
		expect(() =>
			Schema.decodeUnknownSync(ToolchainGeneration)({
				binary: "codex.exe",
				directory: "0.145.0-unique-generation",
				version: "0.145.0",
			}),
		).toThrow();
	});
});
