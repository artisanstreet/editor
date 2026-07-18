import { readFileSync } from "node:fs";
import { basename, join } from "node:path";

import { describe, expect, layer } from "@effect/vitest";
import { Context, Effect, Layer } from "effect";

const component_root = join(process.cwd(), "modules/frontend/src/routes/components");
const source_paths = [
	join(component_root, "editor-shell.sv"),
	join(component_root, "left-pane.sv"),
	join(component_root, "main-pane.sv"),
	join(component_root, "right-pane.sv"),
	join(component_root, "mode-switcher.sv"),
	join(component_root, "quick-open.sv"),
	join(component_root, "file-tab-strip.sv"),
] as const;

interface WorkspaceSource {
	readonly name: string;
	readonly source: string;
}

class WorkspaceSources extends Context.Service<
	WorkspaceSources,
	{ readonly aggregate: string; readonly sources: ReadonlyArray<WorkspaceSource> }
>()("Artisan/Test/WorkspaceSources") {}

const WorkspaceSourcesLive = Layer.effect(
	WorkspaceSources,
	Effect.gen(function* () {
		const sources = yield* Effect.forEach(source_paths, (path) =>
			Effect.try({
				try: () => ({ name: basename(path), source: readFileSync(path, "utf8") }),
				catch: (cause) => cause,
			}),
		);

		return WorkspaceSources.of({
			aggregate: sources.map((source) => source.source).join("\n"),
			sources,
		});
	}),
);

const SourceNamed = (sources: ReadonlyArray<WorkspaceSource>, name: string) =>
	Effect.sync(() => {
		const source = sources.find((candidate) => candidate.name === name);
		if (source === undefined) throw new Error(`Missing workspace source ${name}`);
		return source.source;
	});

describe("production workspace UI source", () => {
	layer(WorkspaceSourcesLive)((it) => {
		it.effect("retains the three-pane shell while composing live projections", () =>
			Effect.gen(function* () {
				const { sources } = yield* WorkspaceSources;
				const shell = yield* SourceNamed(sources, "editor-shell.sv");

				expect(shell).toContain("LiveWorkspaceStore");
				expect(shell).toContain("<LeftPane");
				expect(shell).toContain("<MainPane");
				expect(shell).toContain("<RightPane");
				expect(shell).toContain("<Sheet");
			}),
		);

		it.effect("keeps the editor interaction scaffold reachable without fixture records", () =>
			Effect.gen(function* () {
				const { sources } = yield* WorkspaceSources;
				const main = yield* SourceNamed(sources, "main-pane.sv");

				expect(main).toContain("<QuickOpen");
				expect(main).toContain("<FileTabStrip");
				expect(main).toContain("<WorkspaceNavigation");
				expect(main).toContain("editor-viewport");
				expect(main).toContain("authoritative workspace projection");
			}),
		);

		it.effect("renders truthful live and unavailable states without fixture records", () =>
			Effect.gen(function* () {
				const { aggregate, sources } = yield* WorkspaceSources;
				const main = yield* SourceNamed(sources, "main-pane.sv");
				const left = yield* SourceNamed(sources, "left-pane.sv");
				const right = yield* SourceNamed(sources, "right-pane.sv");

				expect(main).toContain("authoritative workspace projection");
				expect(main).toContain("authoritative orchestration projection");
				expect(left).toContain("No backend threads yet.");
				expect(right).toContain("authoritative workspace projections");
				expect(aggregate).not.toMatch(/editor-fixtures|fixture/i);
			}),
		);

		it.effect("keeps keyboard sending Effect-owned and prevents the composing newline", () =>
			Effect.gen(function* () {
				const { sources } = yield* WorkspaceSources;
				const main = yield* SourceNamed(sources, "main-pane.sv");

				expect(main).toContain("event?.preventDefault()");
				expect(main).toContain("event.metaKey || event.ctrlKey");
				expect(main).toContain("yield* on_send_live_message(chat_draft)");
			}),
		);

		it.effect("keeps component behavior in SER without browser-side runners", () =>
			Effect.gen(function* () {
				const { aggregate } = yield* WorkspaceSources;

				expect(aggregate).not.toMatch(/Effect\.run\w*/);
				expect(aggregate).not.toContain("setTimeout(");
				expect(aggregate).not.toContain("new Promise");
				expect(aggregate).not.toContain("onMount(");
			}),
		);
	});
});
