import { readFile } from "node:fs/promises";
import { describe, expect, it } from "vitest";

const ReadSource = (path: string) => readFile(new URL(`../../${path}`, import.meta.url), "utf8");

const RouteConsumers = [
	"modules/frontend/src/routes/+page.svelte",
	"modules/frontend/src/routes/components/thread-route.svelte",
	"modules/frontend/src/routes/components/sectioned-panel.svelte",
	"modules/frontend/src/routes/components/editor-file-panel.svelte",
	"modules/frontend/src/routes/e/[workspace]/[thread]/editor-route-gate.svelte",
] as const;

describe("route navigation", () => {
	it("centralizes SvelteKit navigation behind a typed browser service", async () => {
		const source = await ReadSource("modules/frontend/src/lib/browser/route-navigation.ts");
		const runtime = await ReadSource(
			"modules/frontend/src/lib/runtime/browser-frontend-runtime.ts",
		);

		expect(source).toContain("Context.Service<");
		expect(source).toContain('Data.TaggedError("RouteNavigationFailure")');
		expect(source).toContain("Effect.gen(function* ()");
		expect(source).toContain("Effect.tryPromise({");
		expect(runtime).toContain("RouteNavigationLive,");
	});

	it("keeps routed components on the capability instead of importing goto", async () => {
		for (const path of RouteConsumers) {
			const source = await ReadSource(path);

			expect(source).toContain("RouteNavigation");
			expect(source).toContain("navigation.Navigate(");
			expect(source).not.toContain('from "$app/navigation"');
		}
	});

	it("runs file-tree callbacks as direct SER event effects", async () => {
		const source = await ReadSource(
			"modules/frontend/src/routes/components/workspace-file-tree.svelte",
		);

		expect(source).toContain('<script lang="ts" effect>');
		expect(source).toContain("Effect.Effect<void>");
		expect(source).toContain("onclick={yield* ontoggle(entry.path)}");
		expect(source).toContain("onclick={yield* onopen(entry.path)}");
	});
});
