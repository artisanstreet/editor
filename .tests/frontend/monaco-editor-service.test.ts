import { describe, expect, it } from "vitest";
import { Effect, Layer, Option } from "effect";

import {
	MakeMonacoEditorLayer,
	MonacoEditorService,
	MonacoUriForFile,
	type MonacoAdapter,
	type MonacoDiagnostic,
	type MonacoEditor,
	type MonacoModel,
	type MonacoViewState,
	type MonacoWorkspaceFile,
} from "../../modules/frontend/src/lib/editor/monaco-editor-service";

class FakeModel implements MonacoModel {
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

class FakeEditor implements MonacoEditor {
	disposed = false;
	model: MonacoModel | undefined;
	restored: MonacoViewState | undefined;
	saved: MonacoViewState | undefined;

	dispose = () => {
		this.disposed = true;
	};
	get_model = () => this.model;
	restore_view_state = (state: MonacoViewState) => {
		this.restored = state;
	};
	save_view_state = () => this.saved;
	set_model = (model: MonacoModel | undefined) => {
		this.model = model;
	};
}

class FakeMonacoAdapter {
	readonly editor = new FakeEditor();
	readonly markers = new Map<string, ReadonlyArray<MonacoDiagnostic>>();
	readonly models: Array<FakeModel> = [];

	readonly adapter: MonacoAdapter = {
		create_editor: () => this.editor,
		create_model: ({ uri, value }) => {
			const model = new FakeModel(uri, value);
			this.models.push(model);
			return model;
		},
		set_markers: (model, _owner, diagnostics) => {
			this.markers.set(model.uri, diagnostics);
		},
	};
}

const FileA: MonacoWorkspaceFile = {
	content: "export const a = 1;",
	id: "file-a",
	language: "typescript",
	path: "src/a.ts",
	revision: "v1",
};

const FileB: MonacoWorkspaceFile = {
	content: "export const b = 2;",
	id: "file-b",
	language: "typescript",
	path: "src/b.ts",
	revision: "v1",
};

describe("Monaco editor service", () => {
	it("keeps unique workspace-file model identities even for matching basenames", async () => {
		expect(MonacoUriForFile(FileA)).not.toBe(
			MonacoUriForFile({ ...FileA, id: "elsewhere", path: "elsewhere/a.ts" }),
		);
	});

	it("preserves a tab view state when switching models", async () => {
		const fake = new FakeMonacoAdapter();
		const state = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const service = yield* MonacoEditorService;
					yield* service.Attach({} as HTMLElement);
					yield* service.Activate(FileA);
					fake.editor.saved = { opaque: { top: 128 } };
					yield* service.Activate(FileB);
					yield* service.Activate(FileA);
					return yield* service.Current;
				}),
			).pipe(Effect.provide(MakeMonacoEditorLayer(fake.adapter))),
		);

		expect(fake.models).toHaveLength(2);
		expect(fake.editor.restored).toEqual({ opaque: { top: 128 } });
		expect(Option.getOrUndefined(state.active_file_id)).toBe(FileA.id);
	});

	it("reports conflicts without invoking persistence and saves current model text", async () => {
		const fake = new FakeMonacoAdapter();
		const persisted: Array<MonacoWorkspaceFile> = [];
		const outcome = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const service = yield* MonacoEditorService;
					yield* service.Activate(FileA);
					yield* service.Update(FileA.id, "export const a = 3;");
					const conflict = yield* service.Save(FileA.id, "stale", (file) =>
						Effect.sync(() => {
							persisted.push(file);
							return { _tag: "Saved" as const, file };
						}),
					);
					const saved = yield* service.Save(FileA.id, "v1", (file) =>
						Effect.sync(() => {
							persisted.push(file);
							return { _tag: "Saved" as const, file };
						}),
					);
					return { conflict, saved };
				}),
			).pipe(Effect.provide(MakeMonacoEditorLayer(fake.adapter))),
		);

		expect(outcome.conflict._tag).toBe("Conflict");
		expect(outcome.saved).toMatchObject({
			_tag: "Saved",
			file: { content: "export const a = 3;" },
		});
		expect(persisted).toHaveLength(1);
	});

	it("applies diagnostics and deterministically disposes every editor resource", async () => {
		const fake = new FakeMonacoAdapter();
		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const service = yield* MonacoEditorService;
					yield* service.Attach({} as HTMLElement);
					yield* service.Activate(FileA);
					yield* service.Mark(FileA.id, [
						{
							end_column: 3,
							end_line: 1,
							message: "bad",
							severity: "error",
							start_column: 1,
							start_line: 1,
						},
					]);
				}),
			).pipe(Effect.provide(MakeMonacoEditorLayer(fake.adapter))),
		);

		expect(fake.markers.get(MonacoUriForFile(FileA))).toHaveLength(1);
		expect(fake.editor.disposed).toBe(true);
		expect(fake.models.every((model) => model.disposed)).toBe(true);
	});
});
