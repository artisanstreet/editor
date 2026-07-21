import { readFileSync, readdirSync } from "node:fs";
import { extname, join, relative, resolve } from "node:path";

import { describe, expect, layer } from "@effect/vitest";
import { Context, Effect, Layer } from "effect";

const frontend_source = resolve("modules/frontend/src");
const source_extensions = new Set([".css", ".html", ".sv", ".svelte", ".ts"]);

interface FrontendSource {
	readonly file: string;
	readonly source: string;
}

class FrontendSources extends Context.Service<
	FrontendSources,
	{
		readonly aggregate: string;
		readonly sources: ReadonlyArray<FrontendSource>;
	}
>()("Artisan/Test/FrontendSources") {}

const SourceFiles = (directory: string): Effect.Effect<ReadonlyArray<string>, unknown> =>
	Effect.gen(function* () {
		const entries = yield* Effect.try(() => readdirSync(directory, { withFileTypes: true }));
		const files: Array<string> = [];

		for (const entry of entries) {
			const path = join(directory, entry.name);

			if (entry.isDirectory()) {
				files.push(...(yield* SourceFiles(path)));
			} else if (source_extensions.has(extname(entry.name))) {
				files.push(path);
			}
		}

		return files;
	});

const FrontendSourcesLive = Layer.effect(
	FrontendSources,
	Effect.gen(function* () {
		const files = yield* SourceFiles(frontend_source);
		const sources: Array<FrontendSource> = [];
		const aggregate: Array<string> = [];

		for (const file of files) {
			const source = yield* Effect.try(() => readFileSync(file, "utf8"));

			sources.push({ file, source });
			aggregate.push(source);
		}

		return FrontendSources.of({ aggregate: aggregate.join("\n"), sources });
	}),
);

const ExpectSource = (sources: ReadonlyArray<FrontendSource>, pattern: RegExp, label: string) =>
	Effect.gen(function* () {
		yield* Effect.void;

		const matches: Array<string> = [];

		for (const source of sources) {
			if (pattern.test(source.source)) {
				matches.push(relative(frontend_source, source.file));
			}
		}

		expect(matches, `Expected frontend source to implement ${label}`).not.toEqual([]);
	});

describe("three-pane shell source layout", () => {
	layer(FrontendSourcesLive)((it) => {
		it.effect("loads and saves desktop pane preferences through the Effect Service", () =>
			Effect.gen(function* () {
				const { aggregate } = yield* FrontendSources;

				expect(aggregate).toContain("yield* ShellPresentationPreferences");
				expect(aggregate).toContain("yield* shell_presentation_preferences.Load");
				expect(aggregate).toContain("shell_presentation_preferences.Save");
				expect(aggregate).toContain("DefaultShellPresentationState");
				expect(aggregate).not.toMatch(/Effect\.run\w*/);
			}),
		);

		it.effect("defines the exact desktop grid and viewport ownership", () =>
			Effect.gen(function* () {
				const { aggregate, sources } = yield* FrontendSources;

				expect(aggregate).toContain("272px minmax(720px, 1fr) 340px");
				yield* ExpectSource(sources, /100dvh/, "a dynamic-viewport-height shell");
				yield* ExpectSource(sources, /editor-shell/, "the editor shell boundary");
			}),
		);

		it.effect("collapses the right pane before the left pane becomes a rail", () =>
			Effect.gen(function* () {
				const { aggregate, sources } = yield* FrontendSources;

				expect(aggregate).toContain("max-width: 1367px");
				yield* ExpectSource(
					sources,
					/@media\s*\([^)]*max-width:\s*1279px[^)]*\)/,
					"the right-pane collapse breakpoint",
				);
				expect(aggregate).toContain("max-width: 999px");
				expect(aggregate).toMatch(/max-width:\s*1279px[\s\S]*?right-slot/);
				expect(aggregate).toMatch(/max-width:\s*999px[\s\S]*?left-rail-slot/);
			}),
		);

		it.effect("provides mobile overlays for both secondary panes", () =>
			Effect.gen(function* () {
				const { aggregate, sources } = yield* FrontendSources;

				yield* ExpectSource(
					sources,
					/@media\s*\([^)]*max-width:\s*(?:999|799)px[^)]*\)/,
					"a mobile overlay breakpoint",
				);
				expect(aggregate).toContain("Open thread navigation");
				expect(aggregate).toContain("Open session pane");
				expect(aggregate).toContain("<Sheet");
				expect(aggregate).toContain("<SheetContent");
				expect(aggregate).toContain("data-open");
				expect(aggregate).toContain("const ClosePanes = Effect.gen");
				expect(aggregate).not.toMatch(/ClosePanes[\s\S]{0,160}\.Save\(/);
			}),
		);

		it.effect("persists explicit desktop collapse without persisting overlay closure", () =>
			Effect.gen(function* () {
				const { aggregate } = yield* FrontendSources;

				expect(aggregate).toContain("data-left-collapsed={left_collapsed}");
				expect(aggregate).toContain("data-right-collapsed={right_collapsed}");
				expect(aggregate).toContain("const CollapseLeft = Effect.gen");
				expect(aggregate).toContain("const CollapseRight = Effect.gen");
				expect(aggregate).toContain("const ExpandLeft = Effect.gen");
				expect(aggregate).toContain("const ExpandRight = Effect.gen");
				expect(aggregate).toMatch(/CollapseLeft[\s\S]{0,180}yield\* SavePresentation/);
				expect(aggregate).toMatch(/CollapseRight[\s\S]{0,180}yield\* SavePresentation/);
				expect(aggregate).toMatch(/ExpandLeft[\s\S]{0,140}yield\* SavePresentation/);
				expect(aggregate).toMatch(/ExpandRight[\s\S]{0,140}yield\* SavePresentation/);
				expect(aggregate).toContain('aria-label="Collapse thread navigation"');
				expect(aggregate).toContain('aria-label="Collapse session pane"');
				expect(aggregate).toMatch(/max-width:\s*1279px[\s\S]*?desktop-right-slot/);
			}),
		);

		it.effect(
			"uses the compact rail and removes the right column for desktop collapse states",
			() =>
				Effect.gen(function* () {
					const { aggregate } = yield* FrontendSources;

					expect(aggregate).toContain('data-left-collapsed="true"');
					expect(aggregate).toContain('data-right-collapsed="true"');
					expect(aggregate).toContain("56px minmax(720px, 1fr) 340px");
					expect(aggregate).toContain("272px minmax(720px, 1fr)");
					expect(aggregate).toContain("56px minmax(720px, 1fr) 280px");
					expect(aggregate).toContain("240px minmax(720px, 1fr)");
					expect(aggregate).toMatch(/data-left-collapsed[\s\S]*?left-rail-slot/);
					expect(aggregate).toMatch(/data-right-collapsed[\s\S]*?desktop-right-slot/);
				}),
		);

		it.effect("reserves inherited header space for exactly the visible pane actions", () =>
			Effect.gen(function* () {
				const { aggregate } = yield* FrontendSources;

				expect(aggregate).toContain("--pane-action-space: 10px");
				expect(aggregate).toContain("--pane-action-space: 48px");
				expect(aggregate).toContain("--pane-action-space: 82px");
				expect(aggregate).toContain("compact-pane-actions");
			}),
		);

		it.effect("exposes labelled regions and independently named pane content", () =>
			Effect.gen(function* () {
				const { aggregate, sources } = yield* FrontendSources;

				for (const label of ["Thread navigation", "Workspace", "Session"]) {
					expect(aggregate).toMatch(
						new RegExp(`<(?:aside|main|section)[^>]*aria-label=["']${label}["']`),
					);
				}
				yield* ExpectSource(
					sources,
					/thread-list/,
					"the independently scrollable thread list",
				);
			}),
		);

		it.effect("keeps workspace modes separate from file tabs", () =>
			Effect.gen(function* () {
				const { aggregate } = yield* FrontendSources;

				expect(aggregate).toMatch(/workspace-mode-switcher/);
				expect(aggregate).toMatch(/file-tab-strip/);
				expect(aggregate.match(/role=["']group["']/g)?.length ?? 0).toBeGreaterThanOrEqual(
					2,
				);
			}),
		);

		it.effect("keeps previews external-only and supplies a reduced-motion state", () =>
			Effect.gen(function* () {
				const { aggregate, sources } = yield* FrontendSources;

				expect(aggregate).toContain("External previews");
				expect(aggregate).toContain("LaunchPreviewInExternalBrowser");
				expect(aggregate).not.toMatch(/<iframe|<webview/i);
				yield* ExpectSource(
					sources,
					/prefers-reduced-motion:\s*reduce/,
					"the reduced-motion override",
				);
			}),
		);

		it.effect("contains no Barekey names or copied asset references", () =>
			Effect.gen(function* () {
				const { sources } = yield* FrontendSources;
				const violations: Array<string> = [];

				for (const source of sources) {
					if (/barekey|usebarekey/i.test(`${source.file}\n${source.source}`)) {
						violations.push(relative(frontend_source, source.file));
					}
				}

				expect(violations).toEqual([]);
			}),
		);
	});
});
