import { globSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const workspace = resolve(import.meta.dirname, "../..");
const source_root = "modules/frontend/src";

/**
 * Svelte templates are outside `tsc`'s reach, so a component that is rendered
 * but never brought into scope type-checks and builds clean, then fails at
 * runtime by silently rendering nothing — taking its whole subtree with it.
 * Renaming or deleting a component is exactly when that happens, and exactly
 * when a reader is least likely to notice.
 */
const scripts_of = (source: string): string =>
	[...source.matchAll(/<script[\s\S]*?<\/script>/gu)].map((match) => match[0]).join("\n");

const template_of = (source: string): string => source.replace(/<script[\s\S]*?<\/script>/gu, "");

/** Every capitalised tag a template renders, namespaced members excluded. */
export const RenderedComponents = (source: string): ReadonlyArray<string> => [
	...new Set(
		[...template_of(source).matchAll(/<([A-Z][A-Za-z0-9]*)(?=[\s/>])/gu)].map(
			(match) => match[1]!,
		),
	),
];

/**
 * A name is in scope when a script mentions it or the template declares it
 * inline: `{@const MarkIcon = …}` is how this codebase resolves a component
 * chosen per row, while snippet parameters may receive component values from
 * their caller.
 */
export const ComponentsInScope = (source: string): ReadonlySet<string> => {
	const scripts = scripts_of(source);
	const inline = [...template_of(source).matchAll(/\{@const\s+([A-Z][A-Za-z0-9]*)\s*=/gu)].map(
		(match) => match[1]!,
	);
	const snippets = [...source.matchAll(/\{#snippet\s+([A-Za-z_][A-Za-z0-9_]*)/gu)].map(
		(match) => match[1]!,
	);
	const snippet_parameters = [
		...template_of(source).matchAll(/\{#snippet\s+[A-Za-z_][A-Za-z0-9_]*\s*\(([^)]*)\)/gu),
	].flatMap((match) =>
		[...match[1]!.matchAll(/\b([A-Z][A-Za-z0-9]*)\b/gu)].map((parameter) => parameter[1]!),
	);
	const named = [...scripts.matchAll(/\b([A-Z][A-Za-z0-9]*)\b/gu)].map((match) => match[1]!);

	return new Set([...inline, ...snippets, ...snippet_parameters, ...named]);
};

describe("component reference integrity", () => {
	const files = globSync(`${source_root}/**/*.sv`, { cwd: workspace });

	it("finds components to check", () => {
		expect(files.length).toBeGreaterThan(20);
	});

	it("brings every component it renders into scope", () => {
		const dangling: Array<string> = [];

		for (const file of files) {
			const source = readFileSync(resolve(workspace, file), "utf8");
			const in_scope = ComponentsInScope(source);

			for (const tag of RenderedComponents(source)) {
				if (in_scope.has(tag)) continue;

				dangling.push(
					`${file.replaceAll("\\", "/")} renders <${tag}> without importing it`,
				);
			}
		}

		expect(dangling).toEqual([]);
	});

	/** The detector is only worth having if it fails on the mistake it exists for. */
	it("reports a component that is rendered but never imported", () => {
		const source = [
			'<script lang="ts">',
			'\timport Kept from "./kept.sv";',
			"</script>",
			"",
			"<Kept />",
			"<Removed />",
		].join("\n");

		expect(RenderedComponents(source)).toEqual(["Kept", "Removed"]);
		expect(ComponentsInScope(source).has("Kept")).toBe(true);
		expect(ComponentsInScope(source).has("Removed")).toBe(false);
	});

	it("accepts a component supplied as a snippet parameter without hiding dangling tags", () => {
		const source = [
			"{#snippet control(Icon)}",
			"\t<Icon />",
			"\t<Removed />",
			"{/snippet}",
		].join("\n");

		expect(ComponentsInScope(source).has("Icon")).toBe(true);
		expect(ComponentsInScope(source).has("Removed")).toBe(false);
	});
});
