import { describe, expect, it } from "vitest";
import { Effect, Option } from "effect";

import {
	MakeEditorLayer,
	EditorService,
	EditorFileKeyForFile,
	EditorUriForFile,
	type EditorAdapter,
	type EditorDiagnostic,
	type EditorSurface,
	type EditorDocument,
	type EditorViewState,
	type EditorWorkspaceFile,
} from "../../modules/frontend/src/lib/editor/service";

class FakeDocument implements EditorDocument {
	disposed = false;
	fail_dispose = false;
	value: string;

	constructor(
		readonly uri: string,
		value: string,
	) {
		this.value = value;
	}

	dispose = () => {
		this.disposed = true;
		if (this.fail_dispose) throw new Error(`model disposal failed: ${this.uri}`);
	};
	get_value = () => this.value;
	set_value = (value: string) => {
		this.value = value;
	};
}

class FakeSurface implements EditorSurface {
	disposed = false;
	fail_dispose = false;
	fail_set_document = false;
	model: EditorDocument | undefined;
	restored: EditorViewState | undefined;
	saved: EditorViewState | undefined;

	dispose = () => {
		this.disposed = true;
		if (this.fail_dispose) throw new Error("editor disposal failed");
	};
	get_document = () => this.model;
	restore_view_state = (state: EditorViewState) => {
		this.restored = state;
	};
	save_view_state = () => this.saved;
	set_document = (model: EditorDocument | undefined) => {
		this.model = model;
		if (this.fail_set_document) throw new Error("set model failed");
	};
}

class FakeSurfaceAdapter {
	readonly surfaces: Array<FakeSurface> = [];
	readonly markers = new Map<string, ReadonlyArray<EditorDiagnostic>>();
	readonly documents: Array<FakeDocument> = [];
	readonly documents_by_uri = new Map<string, FakeDocument>();
	fail_next_surface = false;
	fail_marker_uris = new Set<string>();

	readonly adapter: EditorAdapter = {
		create_surface: () => {
			if (this.fail_next_surface) {
				this.fail_next_surface = false;
				throw new Error("editor creation failed");
			}
			const editor = new FakeSurface();
			this.surfaces.push(editor);
			return editor;
		},
		create_document: ({ uri, value }) => {
			if (this.documents_by_uri.has(uri)) throw new Error(`duplicate URI: ${uri}`);
			const model = new FakeDocument(uri, value);
			this.documents.push(model);
			this.documents_by_uri.set(uri, model);
			return model;
		},
		set_markers: (model, _owner, diagnostics) => {
			this.markers.set(model.uri, diagnostics);
			if (this.fail_marker_uris.has(model.uri)) {
				throw new Error(`marker cleanup failed: ${model.uri}`);
			}
		},
	};
}

const FileA: EditorWorkspaceFile = {
	content: "export const a = 1;",
	id: "file-a",
	language: "typescript",
	path: "src/a.ts",
	revision: "v1",
	workspace_id: "workspace-one",
};

const FileB: EditorWorkspaceFile = {
	content: "export const b = 2;",
	id: "file-b",
	language: "typescript",
	path: "src/b.ts",
	revision: "v1",
	workspace_id: "workspace-one",
};

const Scoped = <A, Error>(
	fake: FakeSurfaceAdapter,
	program: Effect.Effect<A, Error, EditorService>,
) => Effect.runPromise(Effect.scoped(program).pipe(Effect.provide(MakeEditorLayer(fake.adapter))));

describe("editor service", () => {
	it("keeps global editor URI and model identities distinct across workspaces", async () => {
		const second_workspace_file = {
			...FileA,
			path: "src/a.ts",
			workspace_id: "workspace-two",
		};
		expect(EditorUriForFile(FileA)).not.toBe(EditorUriForFile(second_workspace_file));
		expect(EditorFileKeyForFile(FileA)).not.toBe(EditorFileKeyForFile(second_workspace_file));

		const fake = new FakeSurfaceAdapter();
		await Scoped(
			fake,
			Effect.gen(function* () {
				const service = yield* EditorService;
				yield* service.Activate(FileA);
				yield* service.Activate(second_workspace_file);
			}),
		);
		expect(fake.documents).toHaveLength(2);
	});

	it("preserves a tab view state when switching models and re-attaching", async () => {
		const fake = new FakeSurfaceAdapter();
		await Scoped(
			fake,
			Effect.gen(function* () {
				const service = yield* EditorService;
				yield* service.Attach({});
				yield* service.Activate(FileA);
				fake.surfaces[0]!.saved = { opaque: { top: 128 } };
				yield* service.Activate(FileB);
				yield* service.Activate(FileA);
				yield* service.Attach({});
			}),
		);

		expect(fake.documents).toHaveLength(2);
		expect(fake.surfaces[1]!.model?.uri).toBe(EditorUriForFile(FileA));
		expect(fake.surfaces[1]!.restored).toEqual({ opaque: { top: 128 } });
		expect(fake.surfaces[0]!.disposed).toBe(true);
	});

	it("preserves the active file view state across a direct editor remount", async () => {
		const fake = new FakeSurfaceAdapter();
		await Scoped(
			fake,
			Effect.gen(function* () {
				const service = yield* EditorService;
				yield* service.Attach({});
				yield* service.Activate(FileA);
				fake.surfaces[0]!.saved = { opaque: { top: 256 } };
				yield* service.Attach({});
			}),
		);

		expect(fake.surfaces[0]!.disposed).toBe(true);
		expect(fake.surfaces[1]!.restored).toEqual({ opaque: { top: 256 } });
	});

	it("detaches the browser editor without disposing its models and restores the active view", async () => {
		const fake = new FakeSurfaceAdapter();
		const detached = await Scoped(
			fake,
			Effect.gen(function* () {
				const service = yield* EditorService;
				yield* service.Attach({});
				yield* service.Activate(FileA);
				fake.surfaces[0]!.saved = { opaque: { top: 384 } };
				yield* service.Detach;
				const state = yield* service.Current;
				const model_disposed = fake.documents[0]!.disposed;
				yield* service.Attach({});
				return { model_disposed, state };
			}),
		);

		expect(fake.surfaces[0]!.disposed).toBe(true);
		expect(fake.documents[0]!.disposed).toBe(true);
		expect(detached.model_disposed).toBe(false);
		expect(detached.state.active_file_key).toEqual(Option.some(EditorFileKeyForFile(FileA)));
		expect(detached.state.open_file_keys).toContain(EditorFileKeyForFile(FileA));
		expect(fake.surfaces[1]!.model).toBe(fake.documents[0]);
		expect(fake.surfaces[1]!.restored).toEqual({ opaque: { top: 384 } });
	});

	it("leaves a recoverable detached state when a replacement editor cannot be created", async () => {
		const fake = new FakeSurfaceAdapter();
		await Scoped(
			fake,
			Effect.gen(function* () {
				const service = yield* EditorService;
				yield* service.Attach({});
				yield* service.Activate(FileA);
				fake.fail_next_surface = true;
				yield* service.Attach({}).pipe(
					Effect.matchCause({
						onFailure: () => undefined,
						onSuccess: () => undefined,
					}),
				);
				yield* service.Attach({});
			}),
		);

		expect(fake.surfaces).toHaveLength(2);
		expect(fake.surfaces[1]!.model?.uri).toBe(EditorUriForFile(FileA));
	});

	it("reloads clean remote revisions, clears stale diagnostics, and refuses to overwrite a dirty revision", async () => {
		const fake = new FakeSurfaceAdapter();
		const persisted: Array<EditorWorkspaceFile> = [];
		const outcome = await Scoped(
			fake,
			Effect.gen(function* () {
				const service = yield* EditorService;
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
				const content_after_reload = fake.documents[0]!.value;
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
		expect(fake.documents[0]!.value).toBe("export const a = 3;");
		expect(fake.markers.get(EditorUriForFile(FileA))).toEqual([]);
		expect(outcome.conflict).toMatchObject({
			_tag: "Conflict",
			current_file: { revision: "v2" },
		});
		expect(outcome.save).toMatchObject({ _tag: "Conflict", current_revision: "v2" });
		expect(persisted).toHaveLength(0);
		expect(outcome.state.dirty_file_keys).toContain(EditorFileKeyForFile(FileA));
	});

	it("advances the save baseline only after a matching Saved or Unchanged outcome", async () => {
		const fake = new FakeSurfaceAdapter();
		const outcome = await Scoped(
			fake,
			Effect.gen(function* () {
				const service = yield* EditorService;
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
		expect(outcome.after_saved.dirty_file_keys).not.toContain(EditorFileKeyForFile(FileA));
	});

	it("clears markers and disposes each closed model without tearing down the editor", async () => {
		const fake = new FakeSurfaceAdapter();
		const outcome = await Scoped(
			fake,
			Effect.gen(function* () {
				const service = yield* EditorService;
				yield* service.Attach({});
				yield* service.Activate(FileA);
				yield* service.Mark(FileA, []);
				yield* service.Close(FileA);
				return {
					editor_disposed_before_scope_close: fake.surfaces[0]!.disposed,
					state: yield* service.Current,
				};
			}),
		);

		expect(fake.markers.get(EditorUriForFile(FileA))).toEqual([]);
		expect(fake.documents[0]!.disposed).toBe(true);
		expect(outcome.editor_disposed_before_scope_close).toBe(false);
		expect(fake.surfaces[0]!.disposed).toBe(true);
		expect(Option.isNone(outcome.state.active_file_key)).toBe(true);
		expect(outcome.state.open_file_keys).toHaveLength(0);
	});

	it("attempts all close and scope releases when individual adapter callbacks throw", async () => {
		const fake = new FakeSurfaceAdapter();
		const result = await Scoped(
			fake,
			Effect.gen(function* () {
				const service = yield* EditorService;
				yield* service.Attach({});
				yield* service.Activate(FileA);
				yield* service.Activate(FileB);
				fake.fail_marker_uris.add(EditorUriForFile(FileA));
				fake.documents[1]!.fail_dispose = true;
				return yield* service.Dispose.pipe(Effect.exit);
			}),
		);

		expect(result._tag).toBe("Failure");
		expect(fake.surfaces[0]!.disposed).toBe(true);
		expect(fake.documents[0]!.disposed).toBe(true);
		expect(fake.documents[1]!.disposed).toBe(true);
	});

	it("releases a closed model even when detaching it from the active editor throws", async () => {
		const fake = new FakeSurfaceAdapter();
		const result = await Scoped(
			fake,
			Effect.gen(function* () {
				const service = yield* EditorService;
				yield* service.Attach({});
				yield* service.Activate(FileA);
				fake.surfaces[0]!.fail_set_document = true;
				return yield* service.Close(FileA).pipe(Effect.exit);
			}),
		);

		expect(result._tag).toBe("Failure");
		expect(fake.documents[0]!.disposed).toBe(true);
	});
});
