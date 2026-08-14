import { createRequire } from "node:module";
import { globSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

import { describe, expect, it } from "vitest";

import {
	component_gallery_entries,
	component_gallery_index_for,
	component_gallery_neighbor,
} from "../../modules/frontend/src/routes/debug/components/catalog";
import {
	gallery_change_set,
	gallery_compacted,
	gallery_compacting,
	gallery_context_usage,
	gallery_error_event,
	gallery_file_changes,
	gallery_model_handoff,
	gallery_thread_snapshot,
	gallery_usage_continued,
	gallery_usage_limit,
} from "../../modules/frontend/src/routes/debug/components/fixtures";

const workspace = resolve(import.meta.dirname, "../..");
const read_source = (path: string): string => readFileSync(resolve(workspace, path), "utf8");
const require_from_frontend = createRequire(resolve(workspace, "modules/frontend/package.json"));
const { parse } = (await import(
	pathToFileURL(require_from_frontend.resolve("svelte/compiler")).href
)) as { readonly parse: (source: string, options: { readonly modern: true }) => unknown };
const { transform_svelte_effect } = (await import(
	pathToFileURL(require_from_frontend.resolve("svelte-effect-runtime/runtime/transform")).href
)) as {
	readonly transform_svelte_effect: (
		source: string,
		filename: string,
	) => { readonly code: string };
};

describe("thread component gallery", () => {
	it("parses every Svelte source unrooted by the production stub", () => {
		const sources = globSync("modules/frontend/src/routes/debug/components/**/*.svelte", {
			cwd: workspace,
		});

		expect(sources.length).toBeGreaterThan(0);
		for (const path of sources) {
			const transformed = transform_svelte_effect(read_source(path), path);
			expect(() => parse(transformed.code, { modern: true }), path).not.toThrow();
		}
	});

	it("keeps a unique, intentionally broad thread-component catalog", () => {
		const ids = component_gallery_entries.map((entry) => entry.id);

		expect(component_gallery_entries.length).toBeGreaterThanOrEqual(20);
		expect(new Set(ids).size).toBe(ids.length);
		expect(new Set(component_gallery_entries.map((entry) => entry.group))).toEqual(
			new Set([
				"Thread",
				"Messages",
				"Work",
				"Requests",
				"Recovery",
				"Boundaries",
				"Controls",
			]),
		);
		for (const entry of component_gallery_entries) {
			expect(entry.label).not.toBe("");
			expect(entry.description).not.toBe("");
		}
	});

	it("resolves deep links and loops both arrow directions", () => {
		const first = component_gallery_entries[0];
		const last = component_gallery_entries.at(-1);
		expect(first).toBeDefined();
		expect(last).toBeDefined();
		if (first === undefined || last === undefined) return;

		expect(component_gallery_index_for("usage-limit")).toBe(
			component_gallery_entries.findIndex((entry) => entry.id === "usage-limit"),
		);
		expect(component_gallery_index_for("missing-component")).toBe(0);
		expect(component_gallery_index_for(undefined)).toBe(0);
		expect(component_gallery_neighbor(0, -1)).toBe(last);
		expect(component_gallery_neighbor(component_gallery_entries.length - 1, 1)).toBe(first);
	});

	it("builds protocol-valid mock states for the high-risk cards", () => {
		expect(gallery_thread_snapshot.items.length).toBeGreaterThan(0);
		expect(gallery_change_set.file_ids).toHaveLength(gallery_file_changes.length);
		expect(gallery_usage_limit.interruption.state).toBe("scheduled");
		expect(gallery_usage_limit.interruption.alternatives).toHaveLength(1);
		expect(gallery_usage_continued.interruption.state).toBe("continued");
		expect(gallery_error_event.error?.code).toBe("AE-PROVIDER-201");
		expect(gallery_compacting.state).toBe("started");
		expect(gallery_compacted.state).toBe("completed");
		expect(gallery_model_handoff.type).toBe("model_transition");
		expect(gallery_context_usage.context_tokens).toBeLessThan(
			gallery_context_usage.context_window_tokens ?? 0,
		);
	});

	it("renders every catalog entry through one keyed specimen stage", () => {
		const preview = read_source(
			"modules/frontend/src/routes/debug/components/component-preview.svelte",
		);

		for (const entry of component_gallery_entries) {
			expect(preview, entry.id).toContain(`id === "${entry.id}"`);
		}
		expect(preview).toContain(
			"<ThreadWorkspace snapshot={gallery_thread_snapshot} disabled />",
		);
	});

	it("keeps the route above the Forge gate with labelled deep-link controls", () => {
		const page = read_source("modules/frontend/src/routes/debug/components/+page.svelte");
		const config = read_source("modules/frontend/vite.config.ts");

		expect(page).toContain('import { dev } from "$app/environment"');
		expect(page).toContain('page.url.searchParams.get("component")');
		expect(page).toContain('class="fixed inset-0 z-[60]');
		expect(page).toContain("Previous component:");
		expect(page).toContain("Next component:");
		expect(page).toContain("{#key entry.id}");
		expect(config).toContain('/routes/debug/components/+page.svelte"');
	});
});
