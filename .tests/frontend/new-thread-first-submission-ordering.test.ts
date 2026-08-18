import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

import { describe, expect, it } from "vitest";

const workspace = resolve(import.meta.dirname, "../..");
const route_path = "modules/frontend/src/routes/components/thread-route.svelte";
const route_source = readFileSync(resolve(workspace, route_path), "utf8");
const require_from_frontend = createRequire(resolve(workspace, "modules/frontend/package.json"));
const { transform_svelte_effect } = (await import(
	pathToFileURL(require_from_frontend.resolve("svelte-effect-runtime/runtime/transform")).href
)) as {
	readonly transform_svelte_effect: (
		source: string,
		filename: string,
	) => { readonly code: string };
};

describe("new-thread first-submission startup ordering", () => {
	it("compiles claim and delivery launch into one ordered SER effect", () => {
		expect(route_source).toContain(
			"const ClaimAndDeliverInitialFirstSubmission = Effect.gen(function* () {",
		);
		expect(route_source).toContain("yield* ClaimAndDeliverInitialFirstSubmission;");

		const transformed = transform_svelte_effect(route_source, route_path).code;

		expect(transformed).toContain("yield* ToEffect(ClaimAndDeliverInitialFirstSubmission);");
		expect(transformed).not.toContain("yield* ToEffect(ClaimPendingFirstSubmission);");
	});
});
