import { mkdtemp, mkdir, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Effect, Exit, Option } from "effect";
import { describe, expect, it } from "vitest";

import {
	DecodeForgeProfileArgument,
	DecodeHandoffOutput,
	renderer_url,
	ServeRendererAsset,
} from "../../modules/desktop/src/renderer-host";

describe("desktop renderer host", () => {
	it("decodes exactly one loopback handoff line from ae stdout", async () => {
		const handoff = await Effect.runPromise(
			DecodeHandoffOutput(
				[
					"Forge started",
					'{"endpoint":"http://127.0.0.1:52985/","pair_code":"one-time","version":1}',
				].join("\n"),
			),
		);
		expect(handoff).toEqual({
			endpoint: "http://127.0.0.1:52985/",
			pair_code: "one-time",
			version: 1,
		});
	});

	it("rejects non-loopback, credentialed, and portless handoff endpoints", async () => {
		for (const endpoint of [
			"http://attacker.example:80/",
			"https://127.0.0.1:52985/",
			"http://user:pw@127.0.0.1:52985/",
			"http://127.0.0.1/",
			"file:///C:/x",
		]) {
			const exit = await Effect.runPromiseExit(
				DecodeHandoffOutput(
					JSON.stringify({ endpoint, pair_code: "one-time", version: 1 }),
				),
			);
			expect(Exit.isFailure(exit), endpoint).toBe(true);
		}
	});

	it("builds a fragment-only launch URL that never leaves the app origin", () => {
		expect(
			renderer_url(
				Option.some({
					endpoint: "http://127.0.0.1:52985/",
					pair_code: "code with spaces",
					version: 1,
				}),
			),
		).toBe("artisan://app/#pair=code%20with%20spaces&forge=http%3A%2F%2F127.0.0.1%3A52985%2F");
		expect(renderer_url(Option.none())).toBe("artisan://app/");
	});

	it("accepts only a validated --forge-profile argument", () => {
		expect(DecodeForgeProfileArgument(["editor.exe", "--forge-profile=browser-dev"])).toBe(
			"browser-dev",
		);
		expect(DecodeForgeProfileArgument(["editor.exe"])).toBeUndefined();
		expect(DecodeForgeProfileArgument(["--forge-profile=../escape"])).toBeUndefined();
		expect(DecodeForgeProfileArgument(["--forge-profile=has space"])).toBeUndefined();
		expect(DecodeForgeProfileArgument(["--forge-profile="])).toBeUndefined();
	});

	it("serves bundled assets with an index fallback confined to the payload", async () => {
		const frontend_root = await mkdtemp(join(tmpdir(), "artisan-renderer-"));
		await writeFile(join(frontend_root, "index.html"), "<main>Artisan</main>", "utf8");
		await mkdir(join(frontend_root, "_app"));
		await writeFile(join(frontend_root, "_app", "bundle.js"), "export {}", "utf8");

		const serve = (url: string) => Effect.runPromise(ServeRendererAsset(frontend_root, url));

		const shell = await serve("artisan://app/");
		expect(shell.status).toBe(200);
		expect(shell.headers.get("content-type")).toBe("text/html; charset=utf-8");
		expect(await shell.text()).toBe("<main>Artisan</main>");

		/** Client-side routes reload into the shell exactly like the dev Forge's SPA fallback. */
		const deep_route = await serve("artisan://app/threads/12345");
		expect(deep_route.status).toBe(200);
		expect(await deep_route.text()).toBe("<main>Artisan</main>");

		const script = await serve("artisan://app/_app/bundle.js");
		expect(script.status).toBe(200);
		expect(script.headers.get("content-type")).toBe("text/javascript; charset=utf-8");

		expect((await serve("artisan://app/missing.js")).status).toBe(404);
		expect((await serve("artisan://elsewhere/index.html")).status).toBe(404);
		expect((await serve("artisan://app/..%2f..%2fsecrets.txt")).status).toBe(403);
		expect((await serve("artisan://app/%ZZ")).status).toBe(400);
	});
});
