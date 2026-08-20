import { describe, expect, it } from "vitest";
import { Effect, Option } from "effect";

import {
	EditorFileKeyForFile,
	EditorService,
	MakeEditorLayer,
	type EditorAdapter,
	type EditorDiagnostic,
	type EditorDocument,
	type EditorSurface,
	type EditorViewState,
	type EditorWorkspaceFile,
} from "../../modules/frontend/src/lib/editor/service";

class MemoryDocument implements EditorDocument {
	disposed = false;
	value: string;

	constructor(
		readonly uri: string,
		value: string,
	) {
		this.value = value;
	}

	dispose = () => {
		this.disposed = true;
	};
	get_value = () => this.value;
	set_value = (value: string) => {
		this.value = value;
	};
}

class MemorySurface implements EditorSurface {
	model: EditorDocument | undefined;
	restored: EditorViewState | undefined;
	saved: EditorViewState | undefined;

	dispose = () => undefined;
	get_document = () => this.model;
	restore_view_state = (state: EditorViewState) => {
		this.restored = state;
	};
	save_view_state = () => this.saved;
	set_document = (document: EditorDocument | undefined) => {
		this.model = document;
	};
}

class MemoryAdapter {
	readonly documents: Array<MemoryDocument> = [];
	readonly installed_languages: Array<EditorDocument> = [];
	readonly markers = new Map<EditorDocument, ReadonlyArray<EditorDiagnostic>>();
	readonly surface = new MemorySurface();

	readonly adapter: EditorAdapter = {
		create_document: ({ uri, value }) => {
			const document = new MemoryDocument(uri, value);
			this.documents.push(document);
			return document;
		},
		create_surface: () => this.surface,
		install_language: (document) =>
			Effect.sync(() => {
				this.installed_languages.push(document);
			}),
		set_markers: (document, _owner, diagnostics) => {
			this.markers.set(document, diagnostics);
		},
	};
}

const MakeFile = (
	index: number,
	content = `const file_${index} = ${index};`,
): EditorWorkspaceFile => ({
	content,
	id: `file-${index}`,
	language: "typescript",
	path: `src/file-${index}.ts`,
	revision: "v1",
	workspace_id: "workspace-one",
});

const Scoped = <Value>(
	adapter: MemoryAdapter,
	program: Effect.Effect<Value, unknown, EditorService>,
) =>
	Effect.runPromise(
		Effect.scoped(program).pipe(Effect.provide(MakeEditorLayer(adapter.adapter))),
	);

describe("editor document residency", () => {
	it("compacts over-budget dirty inactive models without losing their content, diagnostics, or view state", async () => {
		const adapter = new MemoryAdapter();
		const files = Array.from({ length: 9 }, (_, index) => MakeFile(index));
		const diagnostic: EditorDiagnostic = {
			end_column: 6,
			end_line: 1,
			message: "still unsaved",
			severity: "warning",
			start_column: 1,
			start_line: 1,
		};
		const outcome = await Scoped(
			adapter,
			Effect.gen(function* () {
				const service = yield* EditorService;
				yield* service.Attach({});
				yield* service.Activate(files[0]!);
				yield* service.Update(files[0]!, "const file_0 = 'unsaved';");
				yield* service.Mark(files[0]!, [diagnostic]);
				adapter.surface.saved = { opaque: { top: 480 } };

				for (const file of files.slice(1)) {
					yield* service.Activate(file);
					yield* service.Update(file, `${file.content}\n// unsaved`);
				}

				const original = adapter.documents[0]!;
				const replacement = adapter.documents.at(-1)!;
				const state_before_reactivation = yield* service.Current;
				yield* service.Activate(files[0]!);
				yield* Effect.yieldNow;
				return {
					active_disposed: (adapter.surface.model as MemoryDocument).disposed,
					dirty_before_reactivation: state_before_reactivation.dirty_file_keys,
					installed_on_reactivation: adapter.installed_languages.includes(replacement),
					markers: adapter.markers.get(replacement),
					original_disposed: original.disposed,
					replacement_uri: replacement.uri,
					replacement_value: replacement.value,
					restored: adapter.surface.restored,
					state: yield* service.Current,
				};
			}),
		);

		expect(outcome.original_disposed).toBe(true);
		expect(outcome.active_disposed).toBe(false);
		expect(outcome.replacement_value).toBe("const file_0 = 'unsaved';");
		expect(outcome.replacement_uri).toMatch(/\/src\/file-0\.ts$/);
		expect(outcome.dirty_before_reactivation).toContain(EditorFileKeyForFile(files[0]!));
		expect(outcome.state.dirty_file_keys).toContain(EditorFileKeyForFile(files[0]!));
		expect(outcome.markers).toEqual([diagnostic]);
		expect(outcome.restored).toEqual({ opaque: { top: 480 } });
		expect(outcome.installed_on_reactivation).toBe(true);
	});

	it("compacts a dirty inactive model when the character budget is exceeded", async () => {
		const adapter = new MemoryAdapter();
		const first = MakeFile(0, "a".repeat(3 * 1024 * 1024));
		const second = MakeFile(1, "b".repeat(2 * 1024 * 1024));
		const outcome = await Scoped(
			adapter,
			Effect.gen(function* () {
				const service = yield* EditorService;
				yield* service.Activate(first);
				yield* service.Update(first, `${first.content}!`);
				const original = adapter.documents[0]!;
				yield* service.Activate(second);
				const replacement = adapter.documents.at(-1)!;
				return {
					original_disposed: original.disposed,
					replacement_value: replacement.value,
					state: yield* service.Current,
				};
			}),
		);

		expect(outcome.original_disposed).toBe(true);
		expect(outcome.replacement_value).toBe(`${first.content}!`);
		expect(outcome.state.dirty_file_keys).toEqual(new Set([EditorFileKeyForFile(first)]));
		expect(outcome.state.active_file_key).toEqual(Option.some(EditorFileKeyForFile(second)));
	});
});
