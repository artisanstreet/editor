import { describe, expect, it } from "vitest";
import { Effect, Layer, Option } from "effect";

import {
	MakeMonacoEditorLayer,
	MonacoEditorService,
	MonacoFileKeyForFile,
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
	readonly editors: Array<FakeEditor> = [];
	readonly markers = new Map<string, ReadonlyArray<MonacoDiagnostic>>();
	readonly models: Array<FakeModel> = [];
	readonly models_by_uri = new Map<string, FakeModel>();
	fail_next_editor = false;

	readonly adapter: MonacoAdapter = {
		create_editor: () => {
			if (this.fail_next_editor) {
				this.fail_next_editor = false;
				throw new Error("editor creation failed");
			}
			const editor = new FakeEditor();
			this.editors.push(editor);
			return editor;
		},
		create_model: ({ uri, value }) => {
			if (this.models_by_uri.has(uri)) throw new Error(`duplicate URI: ${uri}`);
			const model = new FakeModel(uri, value);
			this.models.push(model);
			this.models_by_uri.set(uri, model);
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
	workspace_id: "workspace-one",
};

const FileB: MonacoWorkspaceFile = {
	content: "export const b = 2;",
	id: "file-b",
	language: "typescript",
	path: "src/b.ts",
	revision: "v1",
	workspace_id: "workspace-one",
};

const Scoped = <A>(
	fake: FakeMonacoAdapter,
	program: Effect.Effect<A, never, MonacoEditorService>,
) =>
	Effect.runPromise(
		Effect.scoped(program).pipe(Effect.provide(MakeMonacoEditorLayer(fake.adapter))),
	);

describe("Monaco editor service", () => {
	it("keeps global Monaco URI and model identities distinct across workspaces", async () => {
		const second_workspace_file = {
			...FileA,
			path: "src/a.ts",
			workspace_id: "workspace-two",
		};
		expect(MonacoUriForFile(FileA)).not.toBe(MonacoUriForFile(second_workspace_file));
		expect(MonacoFileKeyForFile(FileA)).not.toBe(MonacoFileKeyForFile(second_workspace_file));

		const fake = new FakeMonacoAdapter();
		await Scoped(
			fake,
			Effect.gen(function* () {
				const service = yield* MonacoEditorService;
				yield* service.Activate(FileA);
				yield* service.Activate(second_workspace_file);
			}),
		);
		expect(fake.models).toHaveLength(2);
	});

	it("preserves a tab view state when switching models and re-attaching", async () => {
		const fake = new FakeMonacoAdapter();
		await Scoped(
			fake,
			Effect.gen(function* () {
				const service = yield* MonacoEditorService;
				yield* service.Attach({});
				yield* service.Activate(FileA);
				fake.editors[0]!.saved = { opaque: { top: 128 } };
				yield* service.Activate(FileB);
				yield* service.Activate(FileA);
				yield* service.Attach({});
			}),
		);

		expect(fake.models).toHaveLength(2);
		expect(fake.editors[1]!.model?.uri).toBe(MonacoUriForFile(FileA));
		expect(fake.editors[1]!.restored).toEqual({ opaque: { top: 128 } });
		expect(fake.editors[0]!.disposed).toBe(true);
	});

	it("leaves a recoverable detached state when a replacement editor cannot be created", async () => {
		const fake = new FakeMonacoAdapter();
		await Scoped(
			fake,
			Effect.gen(function* () {
				const service = yield* MonacoEditorService;
				yield* service.Attach({});
				yield* service.Activate(FileA);
				fake.fail_next_editor = true;
				yield* service.Attach({}).pipe(
					Effect.matchCause({
						onFailure: () => undefined,
						onSuccess: () => undefined,
					}),
				);
				yield* service.Attach({});
			}),
		);

		expect(fake.editors).toHaveLength(2);
		expect(fake.editors[1]!.model?.uri).toBe(MonacoUriForFile(FileA));
	});

	it("reloads clean remote revisions, clears stale diagnostics, and refuses to overwrite a dirty revision", async () => {
		const fake = new FakeMonacoAdapter();
		const persisted: Array<MonacoWorkspaceFile> = [];
		const outcome = await Scoped(
			fake,
			Effect.gen(function* () {
				const service = yield* MonacoEditorService;
				yield* service.Activate(FileA);
				yield* service.Mark(FileA, [
					{
						end_column: 3,
						end_line: 1,
						message: "old diagnostic",
						severity: "error",
						start_column: 1,
						start_line: 1,
					},
				]);
				const reloaded = yield* service.Activate({
					...FileA,
					content: "export const a = 2;",
					revision: "v2",
				});
				const content_after_reload = fake.models[0]!.value;
				yield* service.Update(FileA, "export const a = 3;");
				const conflict = yield* service.Activate({
					...FileA,
					content: "export const a = 4;",
					revision: "v3",
				});
				const save = yield* service.Save(FileA, "v3", (file) =>
					Effect.sync(() => {
						persisted.push(file);
						return { _tag: "Saved" as const, file };
					}),
				);
				return {
					conflict,
					content_after_reload,
					reloaded,
					save,
					state: yield* service.Current,
				};
			}),
		);

		expect(outcome.reloaded._tag).toBe("Activated");
		expect(outcome.content_after_reload).toBe("export const a = 2;");
		expect(fake.models[0]!.value).toBe("export const a = 3;");
		expect(fake.markers.get(MonacoUriForFile(FileA))).toEqual([]);
		expect(outcome.conflict).toMatchObject({
			_tag: "Conflict",
			current_file: { revision: "v2" },
		});
		expect(outcome.save).toMatchObject({ _tag: "Conflict", current_revision: "v2" });
		expect(persisted).toHaveLength(0);
		expect(outcome.state.dirty_file_keys).toContain(MonacoFileKeyForFile(FileA));
	});

	it("advances the save baseline only after a matching Saved or Unchanged outcome", async () => {
		const fake = new FakeMonacoAdapter();
		const outcome = await Scoped(
			fake,
			Effect.gen(function* () {
				const service = yield* MonacoEditorService;
				yield* service.Activate(FileA);
				yield* service.Update(FileA, "export const a = 3;");
				const saved = yield* service.Save(FileA, "v1", (file) =>
					Effect.succeed({
						_tag: "Saved" as const,
						file: { ...file, revision: "v2" },
					}),
				);
				const after_saved = yield* service.Current;
				const unchanged = yield* service.Save(FileA, "v2", (file) =>
					Effect.succeed({ _tag: "Unchanged" as const, file }),
				);
				return { after_saved, saved, unchanged };
			}),
		);

		expect(outcome.saved._tag).toBe("Saved");
		expect(outcome.unchanged._tag).toBe("Unchanged");
		expect(outcome.after_saved.dirty_file_keys).not.toContain(MonacoFileKeyForFile(FileA));
	});

	it("clears markers and disposes each closed model without tearing down the editor", async () => {
		const fake = new FakeMonacoAdapter();
		const outcome = await Scoped(
			fake,
			Effect.gen(function* () {
				const service = yield* MonacoEditorService;
				yield* service.Attach({});
				yield* service.Activate(FileA);
				yield* service.Mark(FileA, []);
				yield* service.Close(FileA);
				return {
					editor_disposed_before_scope_close: fake.editors[0]!.disposed,
					state: yield* service.Current,
				};
			}),
		);

		expect(fake.markers.get(MonacoUriForFile(FileA))).toEqual([]);
		expect(fake.models[0]!.disposed).toBe(true);
		expect(outcome.editor_disposed_before_scope_close).toBe(false);
		expect(fake.editors[0]!.disposed).toBe(true);
		expect(Option.isNone(outcome.state.active_file_key)).toBe(true);
		expect(outcome.state.open_file_keys).toHaveLength(0);
	});
});
