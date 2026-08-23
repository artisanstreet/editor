import { describe, expect, it } from "@effect/vitest";
import { Effect, Layer, Stream } from "effect";

import {
	cursor_certified_version,
	engine_distributions,
	grok_certified_version,
	hermes_certified_commit,
	hermes_certified_version,
	ToolchainReleaseHttp,
} from "@artisan/engines";

const distribution = (engine_id: string) => {
	const item = engine_distributions.find((candidate) => candidate.engine_id === engine_id);
	if (item === undefined) throw new Error(`Missing ${engine_id} distribution`);
	return item;
};

const http = ToolchainReleaseHttp.of({
	Get: (url) =>
		Effect.succeed({
			bytes: new TextEncoder().encode(
				url.includes("cursor.com/install")
					? `$version = '${cursor_certified_version}'`
					: grok_certified_version,
			),
			status: 200,
			url,
		}),
	GetStream: () => Stream.empty,
});

describe("managed ACP distributions", () => {
	it.effect("resolves the certified Grok binary with its pinned digest", () =>
		Effect.gen(function* () {
			const grok = distribution("grok");
			expect(yield* grok.LatestVersion).toBe(grok_certified_version);
			const release = yield* grok.ResolveRelease(grok_certified_version, {
				architecture: "x64",
				platform: "win32",
			});
			expect(release).toMatchObject({
				binary: "grok.exe",
				sha256: "4b924daa801663ea20e96382408b1f2b5ba39efad62c14d20d88618a9eb0be64",
				version: grok_certified_version,
			});
		}).pipe(Effect.provide(Layer.succeed(ToolchainReleaseHttp, http))),
	);

	it.effect("resolves Cursor as a bounded ZIP bundle", () =>
		Effect.gen(function* () {
			const cursor = distribution("cursor");
			expect(yield* cursor.LatestVersion).toBe(cursor_certified_version);
			const release = yield* cursor.ResolveRelease(cursor_certified_version, {
				architecture: "x64",
				platform: "win32",
			});
			expect(release).toMatchObject({
				archive_kind: "zip",
				archive_root: "dist-package",
				artifact_kind: "archive-bundle",
				binary: "cursor-agent.cmd",
				sha256: "0458981ffe0fda840d19b97d7cbcb26832dafcf01a9c229f3fb0e0d233d66c4b",
				version: cursor_certified_version,
			});
		}).pipe(Effect.provide(Layer.succeed(ToolchainReleaseHttp, http))),
	);

	it.effect("resolves Hermes as a checksum-pinned staged installer", () =>
		Effect.gen(function* () {
			const hermes = distribution("hermes");
			expect(yield* hermes.LatestVersion).toBe(hermes_certified_version);
			const release = yield* hermes.ResolveRelease(hermes_certified_version, {
				architecture: "x64",
				platform: "win32",
			});
			expect(release).toMatchObject({
				artifact_kind: "staged-installer",
				binary: "hermes-agent/venv/Scripts/hermes.exe",
				commit: hermes_certified_commit,
				installer_sha256:
					"e7521626d40f2d9fc2c51968244f22b3441dc4d5efebb28a0af4b335e91aecdf",
				version: hermes_certified_version,
			});
			if (release.artifact_kind !== "staged-installer") throw new Error("Expected installer");
			expect(release.stages).toContain("path");
		}).pipe(Effect.provide(Layer.succeed(ToolchainReleaseHttp, http))),
	);
});
