import { Context, Effect, Layer, Option, Ref, Scope } from "effect";

export interface MonacoWorkspaceFile {
	readonly id: string;
	readonly workspace_id: string;
	readonly path: string;
	readonly language: string;
	readonly revision: string;
	readonly content: string;
}

export interface MonacoDiagnostic {
	readonly code?: string;
	readonly end_column: number;
	readonly end_line: number;
	readonly message: string;
	readonly severity: "error" | "warning" | "info" | "hint";
	readonly start_column: number;
	readonly start_line: number;
}

export interface MonacoViewState {
	readonly opaque: unknown;
}

export interface MonacoModel {
	readonly dispose: () => void;
	readonly get_value: () => string;
	readonly set_value: (value: string) => void;
	readonly uri: string;
}

export interface MonacoEditor {
	readonly dispose: () => void;
	readonly get_model: () => MonacoModel | undefined;
	readonly restore_view_state: (state: MonacoViewState) => void;
	readonly save_view_state: () => MonacoViewState | undefined;
	readonly set_model: (model: MonacoModel | undefined) => void;
}

/**
 * Monaco itself is browser-only and deliberately lives behind this small
 * adapter. The service remains deterministic under a fake adapter and never
 * receives a filesystem or Electron capability.
 */
export interface MonacoAdapter {
	readonly create_editor: (host: object) => MonacoEditor;
	readonly create_model: (input: {
		readonly language: string;
		readonly uri: string;
		readonly value: string;
	}) => MonacoModel;
	readonly set_markers: (
		model: MonacoModel,
		owner: string,
		diagnostics: ReadonlyArray<MonacoDiagnostic>,
	) => void;
}

export type MonacoSaveOutcome =
	| { readonly _tag: "Saved"; readonly file: MonacoWorkspaceFile }
	| {
			readonly _tag: "Conflict";
			readonly file: MonacoWorkspaceFile;
			readonly current_revision: string;
	  }
	| { readonly _tag: "Unchanged"; readonly file: MonacoWorkspaceFile };

export interface MonacoEditorState {
	readonly active_file_key: Option.Option<string>;
	readonly dirty_file_keys: ReadonlySet<string>;
	readonly open_file_keys: ReadonlySet<string>;
}

export type MonacoFileReference = Pick<MonacoWorkspaceFile, "id" | "workspace_id">;

export type MonacoActivateOutcome =
	| { readonly _tag: "Activated"; readonly file: MonacoWorkspaceFile }
	| {
			readonly _tag: "Conflict";
			readonly current_file: MonacoWorkspaceFile;
			readonly incoming_file: MonacoWorkspaceFile;
	  };

interface ManagedModel {
	readonly diagnostics: ReadonlyArray<MonacoDiagnostic>;
	readonly file: MonacoWorkspaceFile;
	readonly model: MonacoModel;
	readonly view_state: Option.Option<MonacoViewState>;
}

interface InternalState {
	readonly editor: Option.Option<MonacoEditor>;
	readonly models: ReadonlyMap<string, ManagedModel>;
	readonly active_file_key: Option.Option<string>;
}

const EmptyState: InternalState = {
	editor: Option.none(),
	models: new Map(),
	active_file_key: Option.none(),
};

export const MonacoLanguageForPath = (path: string, declared_language: string) => {
	const extension = path.split(".").at(-1)?.toLowerCase();
	const mapped = new Map<string, string>([
		["css", "css"],
		["html", "html"],
		["htm", "html"],
		["js", "javascript"],
		["json", "json"],
		["jsx", "javascript"],
		["md", "markdown"],
		["mjs", "javascript"],
		["svelte", "html"],
		["ts", "typescript"],
		["tsx", "typescript"],
		["yaml", "yaml"],
		["yml", "yaml"],
	] as const);

	return mapped.get(extension ?? "") ?? declared_language;
};

/** Encodes every runtime workspace boundary into the in-memory model identity. */
export const MonacoFileKeyForFile = (file: MonacoFileReference) =>
	`${encodeURIComponent(file.workspace_id)}:${encodeURIComponent(file.id)}`;

/** A stable URI prevents same-path files in distinct workspaces from sharing Monaco identity. */
export const MonacoUriForFile = (file: Pick<MonacoWorkspaceFile, "id" | "path" | "workspace_id">) =>
	`artisan://workspace/${encodeURIComponent(file.workspace_id)}/${encodeURIComponent(file.id)}/${file.path
		.split("/")
		.map((segment) => encodeURIComponent(segment))
		.join("/")}`;

export class MonacoEditorService extends Context.Service<
	MonacoEditorService,
	{
		readonly Activate: (file: MonacoWorkspaceFile) => Effect.Effect<MonacoActivateOutcome>;
		readonly Attach: (host: object) => Effect.Effect<void>;
		readonly Close: (file: MonacoFileReference) => Effect.Effect<void>;
		readonly Current: Effect.Effect<MonacoEditorState>;
		readonly Dispose: Effect.Effect<void>;
		readonly Mark: (
			file: MonacoFileReference,
			diagnostics: ReadonlyArray<MonacoDiagnostic>,
		) => Effect.Effect<void>;
		readonly Save: (
			file: MonacoFileReference,
			expected_revision: string,
			persist: (input: MonacoWorkspaceFile) => Effect.Effect<MonacoSaveOutcome>,
		) => Effect.Effect<MonacoSaveOutcome>;
		readonly Update: (file: MonacoFileReference, content: string) => Effect.Effect<void>;
	}
>()("Artisan/MonacoEditorService") {}

export const MakeMonacoEditorLayer = (adapter: MonacoAdapter) =>
	Layer.effect(
		MonacoEditorService,
		Effect.gen(function* () {
			const scope = yield* Scope.Scope;
			const state = yield* Ref.make<InternalState>(EmptyState);

			const ReleaseModel = (managed: ManagedModel) =>
				Effect.sync(() => {
					adapter.set_markers(managed.model, "artisan", []);
					managed.model.dispose();
				});

			const Dispose = Ref.modify(state, (current) => [current, EmptyState] as const).pipe(
				Effect.flatMap((current) =>
					Effect.sync(() => Option.getOrUndefined(current.editor)?.dispose()).pipe(
						Effect.andThen(
							Effect.forEach(current.models.values(), ReleaseModel, {
								discard: true,
							}),
						),
					),
				),
			);

			yield* Scope.addFinalizer(scope, Effect.ignore(Dispose));

			const Attach = (host: object) =>
				Effect.gen(function* () {
					const current = yield* Ref.get(state);
					yield* Effect.sync(() => Option.getOrUndefined(current.editor)?.dispose());
					yield* Ref.set(state, { ...current, editor: Option.none() });
					const editor = yield* Effect.sync(() => adapter.create_editor(host));
					const latest = yield* Ref.get(state);
					const active_key = Option.getOrUndefined(latest.active_file_key);
					const active =
						active_key === undefined ? undefined : latest.models.get(active_key);
					yield* Effect.sync(() => {
						editor.set_model(active?.model);
						const view_state =
							active === undefined
								? undefined
								: Option.getOrUndefined(active.view_state);
						if (view_state !== undefined) editor.restore_view_state(view_state);
					});
					yield* Ref.set(state, { ...latest, editor: Option.some(editor) });
				});

			const Activate = (file: MonacoWorkspaceFile) =>
				Effect.gen(function* () {
					const current = yield* Ref.get(state);
					const file_key = MonacoFileKeyForFile(file);
					const existing = current.models.get(file_key);
					const models = new Map(current.models);
					const is_dirty =
						existing !== undefined &&
						existing.model.get_value() !== existing.file.content;
					const outcome: MonacoActivateOutcome =
						existing !== undefined &&
						existing.file.revision !== file.revision &&
						is_dirty
							? {
									_tag: "Conflict",
									current_file: existing.file,
									incoming_file: file,
								}
							: { _tag: "Activated", file };
					const managed =
						existing === undefined
							? {
									diagnostics: [] as ReadonlyArray<MonacoDiagnostic>,
									file,
									model: yield* Effect.sync(() =>
										adapter.create_model({
											language: MonacoLanguageForPath(
												file.path,
												file.language,
											),
											uri: MonacoUriForFile(file),
											value: file.content,
										}),
									),
									view_state: Option.none<MonacoViewState>(),
								}
							: existing.file.revision === file.revision || is_dirty
								? existing
								: yield* Effect.sync(() => {
										existing.model.set_value(file.content);
										adapter.set_markers(existing.model, "artisan", []);
										return {
											diagnostics: [] as ReadonlyArray<MonacoDiagnostic>,
											file,
											model: existing.model,
											view_state: Option.none<MonacoViewState>(),
										};
									});

					const editor = Option.getOrUndefined(current.editor);
					if (editor !== undefined) {
						const previous = Option.getOrUndefined(current.active_file_key);
						if (previous !== undefined && previous !== file_key) {
							const previous_managed = current.models.get(previous);
							const view_state = editor.save_view_state();
							if (previous_managed !== undefined && view_state !== undefined) {
								models.set(previous, {
									...previous_managed,
									view_state: Option.some(view_state),
								});
							}
						}
						editor.set_model(managed.model);
						const view_state = Option.getOrUndefined(managed.view_state);
						if (view_state !== undefined) {
							editor.restore_view_state(view_state);
						}
					}

					models.set(file_key, managed);
					yield* Ref.set(state, {
						...current,
						active_file_key: Option.some(file_key),
						models,
					});
					return outcome;
				});

			const Update = (file: MonacoFileReference, content: string) =>
				Effect.gen(function* () {
					const current = yield* Ref.get(state);
					const managed = current.models.get(MonacoFileKeyForFile(file));
					if (managed === undefined) return;
					yield* Effect.sync(() => managed.model.set_value(content));
				});

			const Mark = (
				file: MonacoFileReference,
				diagnostics: ReadonlyArray<MonacoDiagnostic>,
			) =>
				Effect.gen(function* () {
					const current = yield* Ref.get(state);
					const file_key = MonacoFileKeyForFile(file);
					const managed = current.models.get(file_key);
					if (managed === undefined) return;
					yield* Effect.sync(() =>
						adapter.set_markers(managed.model, "artisan", diagnostics),
					);
					const models = new Map(current.models);
					models.set(file_key, { ...managed, diagnostics });
					yield* Ref.set(state, { ...current, models });
				});

			const Save = (
				file: MonacoFileReference,
				expected_revision: string,
				persist: (input: MonacoWorkspaceFile) => Effect.Effect<MonacoSaveOutcome>,
			) =>
				Effect.gen(function* () {
					const current = yield* Ref.get(state);
					const file_key = MonacoFileKeyForFile(file);
					const managed = current.models.get(file_key);
					if (managed === undefined || managed.file.revision !== expected_revision) {
						return {
							_tag: "Conflict" as const,
							current_revision: managed?.file.revision ?? "unavailable",
							file: managed?.file ?? {
								content: "",
								id: file.id,
								language: "plaintext",
								path: file.id,
								revision: "unavailable",
								workspace_id: file.workspace_id,
							},
						};
					}

					const persisted_file = {
						...managed.file,
						content: managed.model.get_value(),
					};
					const outcome = yield* persist(persisted_file);
					if (outcome._tag === "Saved" || outcome._tag === "Unchanged") {
						yield* Ref.update(state, (latest) => {
							const latest_managed = latest.models.get(file_key);
							if (
								latest_managed?.model !== managed.model ||
								MonacoFileKeyForFile(outcome.file) !== file_key
							)
								return latest;
							const models = new Map(latest.models);
							models.set(file_key, { ...latest_managed, file: outcome.file });
							return { ...latest, models };
						});
					}

					return outcome;
				});

			const Close = (file: MonacoFileReference) =>
				Effect.gen(function* () {
					const file_key = MonacoFileKeyForFile(file);
					const closed = yield* Ref.modify(state, (current) => {
						const managed = current.models.get(file_key);
						if (managed === undefined) return [undefined, current] as const;
						const models = new Map(current.models);
						models.delete(file_key);
						const is_active =
							Option.getOrUndefined(current.active_file_key) === file_key;
						return [
							{ is_active, managed, editor: Option.getOrUndefined(current.editor) },
							{
								...current,
								active_file_key: is_active
									? Option.none()
									: current.active_file_key,
								models,
							},
						] as const;
					});
					if (closed === undefined) return;
					if (closed.is_active) {
						yield* Effect.sync(() => closed.editor?.set_model(undefined));
					}
					yield* ReleaseModel(closed.managed);
				});

			const Current = Ref.get(state).pipe(
				Effect.map((current) => ({
					active_file_key: current.active_file_key,
					dirty_file_keys: new Set(
						[...current.models.entries()]
							.filter(
								([, managed]) => managed.model.get_value() !== managed.file.content,
							)
							.map(([file_id]) => file_id),
					),
					open_file_keys: new Set(current.models.keys()),
				})),
			);

			return MonacoEditorService.of({
				Activate,
				Attach,
				Close,
				Current,
				Dispose,
				Mark,
				Save,
				Update,
			});
		}),
	);
