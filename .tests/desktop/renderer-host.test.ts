import { mkdtemp, mkdir, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Effect, Exit } from "effect";
import { describe, expect, it } from "vitest";

import {
	DecodeHandoffOutput,
	renderer_handoff_url,
	renderer_loader_url,
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

	it("accepts legacy handoffs and constrains optional editor ownership", async () => {
		const legacy = await Effect.runPromise(
			DecodeHandoffOutput(
				'{"endpoint":"http://127.0.0.1:52985/","pair_code":"one-time","version":1}',
			),
		);
		expect(legacy.owned_instance_id).toBeUndefined();

		const current = await Effect.runPromise(
			DecodeHandoffOutput(
				'{"endpoint":"http://127.0.0.1:52985/","owned_instance_id":"forge_owner-1","pair_code":"one-time","version":1}',
			),
		);
		expect(current.owned_instance_id).toBe("forge_owner-1");

		const exit = await Effect.runPromiseExit(
			DecodeHandoffOutput(
				'{"endpoint":"http://127.0.0.1:52985/","owned_instance_id":"forge&stop","pair_code":"one-time","version":1}',
			),
		);
		expect(Exit.isFailure(exit)).toBe(true);
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

	it("forces a document navigation while keeping pairing material in the fragment", () => {
		const url = renderer_handoff_url(
			{
				endpoint: "http://127.0.0.1:52985/",
				pair_code: "code with spaces",
				version: 1,
			},
			7,
		);
		const [request_url, fragment] = url.split("#");
		expect(request_url).toBe("artisan://app/?artisan-handoff=7");
		expect(request_url).not.toContain("code%20with%20spaces");
		expect(request_url).not.toContain("52985");
		expect(fragment).toBe("pair=code%20with%20spaces&forge=http%3A%2F%2F127.0.0.1%3A52985%2F");
		expect(renderer_loader_url).toBe("artisan://app/");
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
		const deep_route = await serve("artisan://app/t/workspace_1/12345");
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
