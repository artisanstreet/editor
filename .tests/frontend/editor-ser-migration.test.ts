import { readFile } from "node:fs/promises";
import { describe, expect, it } from "vitest";

const ReadSource = (path: string) => readFile(new URL(`../../${path}`, import.meta.url), "utf8");

const EditorEffectPaths = [
	"modules/frontend/src/lib/editor/codemirror-adapter.ts",
	"modules/frontend/src/lib/editor/language.ts",
	"modules/frontend/src/lib/editor/service.ts",
	"modules/frontend/src/routes/components/editor-file-panel.sv",
	"modules/frontend/src/routes/e/[workspace]/[thread]/editor-route-gate.sv",
] as const;

const ForbiddenWorkflowConstruction =
	/\bEffect\.(?:run(?:Sync|Fork|Promise)?|sync|succeed|flatMap|andThen|suspend)\b|\bManagedRuntime\b|\bRuntime\.[A-Za-z_]+|\basync\b|\bawait\b|\.then\s*\(|new Promise\b/;

describe("editor SER migration", () => {
	it.each(EditorEffectPaths)("keeps %s on component-owned Effect programs", async (path) => {
		const source = await ReadSource(path);

		expect(source).not.toMatch(ForbiddenWorkflowConstruction);
	});

	it("keeps editor cleanup scoped while retaining independent release failures", async () => {
		const source = await ReadSource("modules/frontend/src/lib/editor/service.ts");

		expect(source).toContain("const scope = yield* Scope.Scope;");
		expect(source).toContain("yield* Scope.addFinalizer(scope, Dispose.pipe(Effect.ignore));");
		expect(source).toContain("const exits = yield* Effect.forEach(releases, (release) =>");
		expect(source).toContain("yield* Effect.failCause(");
		expect(source).toContain("Effect.forkIn(scope)");
		expect(source).toContain("const RunAdapter = <Value>(operation: () => Value) =>");
		expect(source).toContain("new EditorAdapterFailure({");
		expect(source).not.toMatch(/\byield\*\s+Effect\.void\b|\btry\s*\{|\bcatch\s*\{/);
	});

	it("models file-tree work as reactive state instead of an unsafe event executor", async () => {
		const source = await ReadSource(
			"modules/frontend/src/routes/components/editor-file-panel.sv",
		);

		expect(source).toContain("let directory_requests = $state.raw");
		expect(source).toContain("yield* LoadDirectory(parent);");
		expect(source).not.toMatch(/Queue\.|offerUnsafe|\bvoid goto\b/);
	});
});
