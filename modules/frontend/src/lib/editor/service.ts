import { Cause, Context, Data, Effect, Exit, Layer, Option, Ref, Scope } from "effect";

import type {
	EditorAdapter,
	EditorDiagnostic,
	EditorDocument,
	EditorSurface,
	EditorViewState,
	EditorWorkspaceFile,
} from "./adapter";
import { EditorLanguageForPath } from "./language";

export type {
	EditorAdapter,
	EditorDiagnostic,
	EditorDocument,
	EditorSurface,
	EditorViewState,
	EditorWorkspaceFile,
};

/**
 * Owns which workspace files are open, which are dirty, and which one the
 * surface is showing.
 *
 * The service holds no editor implementation of its own: it drives an
 * `EditorAdapter`, so every rule below — conflict detection on activate,
 * revision arbitration on save, view-state hand-off between documents — is
 * exercised in tests against a fake adapter with no browser present.
 */

export type EditorSaveOutcome =
	| { readonly _tag: "Saved"; readonly file: EditorWorkspaceFile }
	| {
			readonly _tag: "Conflict";
			readonly file: EditorWorkspaceFile;
			readonly current_revision: string;
	  }
	| { readonly _tag: "Unchanged"; readonly file: EditorWorkspaceFile };

/** Lets the mounting component learn that the document changed under the user. */
export interface EditorAttachOptions {
	readonly on_change?: () => void;
}

/** A browser editor adapter rejected one narrowly bounded host operation. */
export class EditorAdapterFailure extends Data.TaggedError("EditorAdapterFailure")<{
	readonly cause: unknown;
	readonly message: string;
}> {}

export interface EditorSessionState {
	readonly active_file_key: Option.Option<string>;
	readonly dirty_file_keys: ReadonlySet<string>;
	readonly open_file_keys: ReadonlySet<string>;
}

export type EditorFileReference = Pick<EditorWorkspaceFile, "id" | "workspace_id">;

export type EditorActivateOutcome =
	| { readonly _tag: "Activated"; readonly file: EditorWorkspaceFile }
	| {
			readonly _tag: "Conflict";
			readonly current_file: EditorWorkspaceFile;
			readonly incoming_file: EditorWorkspaceFile;
	  };

interface OpenDocument {
	readonly diagnostics: ReadonlyArray<EditorDiagnostic>;
	readonly file: EditorWorkspaceFile;
	readonly document: EditorDocument;
	/**
	 * A compacted document still owns every unsaved byte, but its previous
	 * adapter model (language workers, parse state, and undo history) has been
	 * released. It becomes hot again only when the user returns to it.
	 */
	readonly residency: "compacted" | "hot";
	readonly view_state: Option.Option<EditorViewState>;
}

interface InternalState {
	readonly surface: Option.Option<EditorSurface>;
	readonly documents: ReadonlyMap<string, OpenDocument>;
	readonly active_file_key: Option.Option<string>;
}

const EmptyState: InternalState = {
	surface: Option.none(),
	documents: new Map(),
	active_file_key: Option.none(),
};

/**
 * The editor service is runtime-long-lived, so inactive clean documents need a
 * bounded resident set. Dirty document content and the active hot model are
 * always protected; inactive dirty adapter state may be compacted without
 * discarding the user's unsaved bytes.
 */
const MaximumRetainedDocuments = 8;
const MaximumRetainedDocumentCharacters = 4 * 1024 * 1024;

/** Encodes every runtime workspace boundary into the in-memory document identity. */
export const EditorFileKeyForFile = (file: EditorFileReference) =>
	`${encodeURIComponent(file.workspace_id)}:${encodeURIComponent(file.id)}`;

/** A stable URI prevents same-path files in distinct workspaces from sharing editor identity. */
export const EditorUriForFile = (file: Pick<EditorWorkspaceFile, "id" | "path" | "workspace_id">) =>
	`artisan://workspace/${encodeURIComponent(file.workspace_id)}/${encodeURIComponent(file.id)}/${file.path
		.split("/")
		.map((segment) => encodeURIComponent(segment))
		.join("/")}`;

export class EditorService extends Context.Service<
	EditorService,
	{
		readonly Activate: (
			file: EditorWorkspaceFile,
		) => Effect.Effect<EditorActivateOutcome, EditorAdapterFailure>;
		readonly Attach: (
			host: object,
			options?: EditorAttachOptions,
		) => Effect.Effect<void, EditorAdapterFailure>;
		readonly Close: (file: EditorFileReference) => Effect.Effect<void, EditorAdapterFailure>;
		readonly Current: Effect.Effect<EditorSessionState, EditorAdapterFailure>;
		readonly Detach: Effect.Effect<void, EditorAdapterFailure>;
		readonly Dispose: Effect.Effect<void, EditorAdapterFailure>;
		readonly Mark: (
			file: EditorFileReference,
			diagnostics: ReadonlyArray<EditorDiagnostic>,
		) => Effect.Effect<void, EditorAdapterFailure>;
		readonly Save: (
			file: EditorFileReference,
			expected_revision: string,
			persist: (input: EditorWorkspaceFile) => Effect.Effect<EditorSaveOutcome>,
		) => Effect.Effect<EditorSaveOutcome, EditorAdapterFailure>;
		readonly Update: (
			file: EditorFileReference,
			content: string,
		) => Effect.Effect<void, EditorAdapterFailure>;
	}
>()("Artisan/EditorService") {}

export const MakeEditorLayer = (adapter: EditorAdapter) =>
	Layer.effect(
		EditorService,
		Effect.gen(function* () {
			const scope = yield* Scope.Scope;
			const state = yield* Ref.make<InternalState>(EmptyState);
			let compacted_document_sequence = 0;
			/** Every mutable adapter boundary is lazy, yielded, and tagged. */
			const RunAdapter = <Value>(operation: () => Value) =>
				Effect.gen(function* () {
					return yield* Effect.try({
						catch: (cause) =>
							new EditorAdapterFailure({
								cause,
								message:
									cause instanceof Error
										? cause.message
										: "Editor adapter operation failed.",
							}),
						try: operation,
					});
				});

			/**
			 * Adapter disposal callbacks are external browser code. Attempt every
			 * independent release and retain all defects instead of letting the first
			 * one strand later documents or surfaces.
			 */
			const ReleaseAll = (
				releases: ReadonlyArray<Effect.Effect<void, EditorAdapterFailure>>,
			): Effect.Effect<void, EditorAdapterFailure> =>
				Effect.gen(function* () {
					const exits = yield* Effect.forEach(releases, (release) =>
						Effect.gen(function* () {
							return yield* Effect.exit(release);
						}),
					);
					const [first_failure, ...remaining_failures] = exits.filter(Exit.isFailure);
					if (first_failure === undefined) return;
					return yield* Effect.failCause(
						remaining_failures.reduce(
							(cause, exit) => Cause.combine(cause, exit.cause),
							first_failure.cause,
						),
					);
				});

			const ReleaseDocument = (managed: OpenDocument) =>
				Effect.gen(function* () {
					yield* ReleaseAll([
						RunAdapter(() => adapter.set_markers(managed.document, "artisan", [])),
						RunAdapter(() => managed.document.dispose()),
					]);
				});

			const SaveViewState = (surface: EditorSurface) =>
				Effect.gen(function* () {
					return yield* RunAdapter(() => surface.save_view_state());
				});

			/**
			 * Map insertion order is the document LRU: activating a document moves it
			 * to the end, and the first clean inactive entry is evicted first. Reading
			 * each value here also makes the character budget reflect unsaved changes.
			 */
			const ReclaimInactiveDocuments = (
				documents: ReadonlyMap<string, OpenDocument>,
				active_file_key: string,
			) =>
				Effect.gen(function* () {
					const protected_keys = new Set<string>([active_file_key]);
					const character_counts = new Map<string, number>();
					let retained_characters = 0;

					for (const [file_key, managed] of documents) {
						const value = yield* RunAdapter(() => managed.document.get_value());
						character_counts.set(file_key, value.length);
						retained_characters += value.length;
						if (value !== managed.file.content) protected_keys.add(file_key);
					}

					const retained = new Map(documents);
					const evicted: Array<OpenDocument> = [];
					const compacted: Array<OpenDocument> = [];
					while (
						retained.size > MaximumRetainedDocuments ||
						retained_characters > MaximumRetainedDocumentCharacters
					) {
						const candidate = [...retained].find(
							([file_key]) => !protected_keys.has(file_key),
						);
						if (candidate === undefined) break;
						const [file_key, managed] = candidate;
						retained.delete(file_key);
						retained_characters -= character_counts.get(file_key) ?? 0;
						evicted.push(managed);
					}

					/**
					 * A dirty document cannot be evicted without data loss. Once clean
					 * candidates are gone, release the oldest inactive *hot* adapter
					 * models instead. The replacement starts with only its current text;
					 * diagnostics and view state live in this service and are restored on
					 * activation. Its compacted residency prevents a budget that remains
					 * over limit because of unsaved text from repeatedly rebuilding it.
					 */
					let hot_document_count = [...retained.values()].filter(
						(managed) => managed.residency === "hot",
					).length;
					let hot_document_characters = [...retained].reduce(
						(total, [file_key, managed]) =>
							total +
							(managed.residency === "hot"
								? (character_counts.get(file_key) ?? 0)
								: 0),
						0,
					);
					while (
						hot_document_count > MaximumRetainedDocuments ||
						hot_document_characters > MaximumRetainedDocumentCharacters
					) {
						const candidate = [...retained].find(
							([file_key, managed]) =>
								file_key !== active_file_key &&
								managed.residency === "hot" &&
								protected_keys.has(file_key),
						);
						if (candidate === undefined) break;
						const [file_key, managed] = candidate;
						const value = yield* RunAdapter(() => managed.document.get_value());
						const document = yield* RunAdapter(() =>
							adapter.create_document({
								language: EditorLanguageForPath(
									managed.file.path,
									managed.file.language,
								),
								/**
								 * A replacement cannot share a still-live adapter URI with the
								 * model it is about to retire. Keeping this identity distinct also
								 * makes compaction safe for adapters that defer registry cleanup
								 * until after disposal returns.
								 */
								uri: EditorUriForFile({
									...managed.file,
									/** Keep the real path suffix so language detection still sees its extension. */
									id: `${managed.file.id}:compacted:${compacted_document_sequence++}`,
								}),
								value,
							}),
						);
						retained.set(file_key, {
							...managed,
							document,
							residency: "compacted",
						});
						hot_document_count -= 1;
						hot_document_characters -= value.length;
						compacted.push(managed);
					}

					return { compacted, documents: retained, evicted };
				});

			const Dispose = Effect.gen(function* () {
				const current = yield* Ref.modify(
					state,
					(current) => [current, EmptyState] as const,
				);
				yield* ReleaseAll([
					RunAdapter(() => Option.getOrUndefined(current.surface)?.dispose()).pipe(
						Effect.asVoid,
					),
					...Array.from(current.documents.values(), ReleaseDocument),
				]);
			});

			yield* Scope.addFinalizer(scope, Dispose.pipe(Effect.ignore));

			const Attach = (host: object, options?: EditorAttachOptions) =>
				Effect.gen(function* () {
					const current = yield* Ref.get(state);
					const active_key = Option.getOrUndefined(current.active_file_key);
					const active =
						active_key === undefined ? undefined : current.documents.get(active_key);
					const previous_surface = Option.getOrUndefined(current.surface);
					const view_state =
						previous_surface === undefined
							? undefined
							: yield* SaveViewState(previous_surface);
					const documents = new Map(current.documents);
					if (
						active_key !== undefined &&
						active !== undefined &&
						view_state !== undefined
					) {
						documents.set(active_key, {
							...active,
							view_state: Option.some(view_state),
						});
					}
					yield* RunAdapter(() => previous_surface?.dispose()).pipe(Effect.asVoid);
					yield* Ref.set(state, { ...current, surface: Option.none(), documents });
					const surface = yield* RunAdapter(() =>
						adapter.create_surface(host, { on_change: () => options?.on_change?.() }),
					);
					const latest = yield* Ref.get(state);
					const latest_active_key = Option.getOrUndefined(latest.active_file_key);
					const latest_active =
						latest_active_key === undefined
							? undefined
							: latest.documents.get(latest_active_key);
					yield* RunAdapter(() => surface.set_document(latest_active?.document));
					const latest_view_state =
						latest_active === undefined
							? undefined
							: Option.getOrUndefined(latest_active.view_state);
					if (latest_view_state !== undefined)
						yield* RunAdapter(() => surface.restore_view_state(latest_view_state));
					yield* Ref.set(state, { ...latest, surface: Option.some(surface) });
				});

			const Detach = Effect.gen(function* () {
				const current = yield* Ref.get(state);
				const surface = Option.getOrUndefined(current.surface);
				if (surface === undefined) return;
				const active_key = Option.getOrUndefined(current.active_file_key);
				const active =
					active_key === undefined ? undefined : current.documents.get(active_key);
				const view_state = yield* SaveViewState(surface);
				const documents = new Map(current.documents);
				if (active_key !== undefined && active !== undefined && view_state !== undefined)
					documents.set(active_key, { ...active, view_state: Option.some(view_state) });
				yield* Ref.set(state, { ...current, surface: Option.none(), documents });
				yield* RunAdapter(() => surface.dispose());
			});

			const Activate = (file: EditorWorkspaceFile) =>
				Effect.gen(function* () {
					const current = yield* Ref.get(state);
					const file_key = EditorFileKeyForFile(file);
					const existing = current.documents.get(file_key);
					const documents = new Map(current.documents);
					const is_dirty =
						existing !== undefined &&
						(yield* RunAdapter(() => existing.document.get_value())) !==
							existing.file.content;
					const outcome: EditorActivateOutcome =
						existing !== undefined &&
						existing.file.revision !== file.revision &&
						is_dirty
							? {
									_tag: "Conflict",
									current_file: existing.file,
									incoming_file: file,
								}
							: { _tag: "Activated", file };
					let managed: OpenDocument;
					if (existing === undefined) {
						managed = {
							diagnostics: [] as ReadonlyArray<EditorDiagnostic>,
							file,
							document: yield* RunAdapter(() =>
								adapter.create_document({
									language: EditorLanguageForPath(file.path, file.language),
									uri: EditorUriForFile(file),
									value: file.content,
								}),
							),
							residency: "hot",
							view_state: Option.none<EditorViewState>(),
						};
					} else if (existing.file.revision === file.revision || is_dirty) {
						managed = { ...existing, residency: "hot" };
					} else {
						yield* RunAdapter(() => existing.document.set_value(file.content));
						yield* RunAdapter(() =>
							adapter.set_markers(existing.document, "artisan", []),
						);
						managed = {
							diagnostics: [] as ReadonlyArray<EditorDiagnostic>,
							file,
							document: existing.document,
							residency: "hot",
							view_state: Option.none<EditorViewState>(),
						};
					}
					if (
						(existing === undefined || existing.residency === "compacted") &&
						adapter.install_language !== undefined
					) {
						yield* adapter
							.install_language(managed.document)
							.pipe(Effect.ignore, Effect.forkIn(scope));
					}
					if (existing?.residency === "compacted" && managed.diagnostics.length > 0) {
						yield* RunAdapter(() =>
							adapter.set_markers(managed.document, "artisan", managed.diagnostics),
						);
					}

					const surface = Option.getOrUndefined(current.surface);
					if (surface !== undefined) {
						const previous = Option.getOrUndefined(current.active_file_key);
						if (previous !== undefined && previous !== file_key) {
							const previous_managed = current.documents.get(previous);
							const view_state = yield* SaveViewState(surface);
							if (previous_managed !== undefined && view_state !== undefined) {
								documents.set(previous, {
									...previous_managed,
									view_state: Option.some(view_state),
								});
							}
						}
						yield* RunAdapter(() => surface.set_document(managed.document));
						const view_state = Option.getOrUndefined(managed.view_state);
						if (view_state !== undefined) {
							yield* RunAdapter(() => surface.restore_view_state(view_state));
						}
					}

					/** Delete before set so activation is a recency touch even for an existing document. */
					documents.delete(file_key);
					documents.set(file_key, managed);
					const retained = yield* ReclaimInactiveDocuments(documents, file_key);
					yield* Ref.set(state, {
						...current,
						active_file_key: Option.some(file_key),
						documents: retained.documents,
					});
					yield* ReleaseAll(
						[...retained.evicted, ...retained.compacted].map(ReleaseDocument),
					);
					return outcome;
				});

			const Update = (file: EditorFileReference, content: string) =>
				Effect.gen(function* () {
					const current = yield* Ref.get(state);
					const managed = current.documents.get(EditorFileKeyForFile(file));
					if (managed === undefined) return;
					yield* RunAdapter(() => managed.document.set_value(content));
				});

			const Mark = (
				file: EditorFileReference,
				diagnostics: ReadonlyArray<EditorDiagnostic>,
			) =>
				Effect.gen(function* () {
					const current = yield* Ref.get(state);
					const file_key = EditorFileKeyForFile(file);
					const managed = current.documents.get(file_key);
					if (managed === undefined) return;
					yield* RunAdapter(() =>
						adapter.set_markers(managed.document, "artisan", diagnostics),
					);
					const documents = new Map(current.documents);
					documents.set(file_key, { ...managed, diagnostics });
					yield* Ref.set(state, { ...current, documents });
				});

			const Save = (
				file: EditorFileReference,
				expected_revision: string,
				persist: (input: EditorWorkspaceFile) => Effect.Effect<EditorSaveOutcome>,
			) =>
				Effect.gen(function* () {
					const current = yield* Ref.get(state);
					const file_key = EditorFileKeyForFile(file);
					const managed = current.documents.get(file_key);
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
						content: yield* RunAdapter(() => managed.document.get_value()),
					};
					const outcome = yield* persist(persisted_file);
					if (outcome._tag === "Saved" || outcome._tag === "Unchanged") {
						yield* Ref.update(state, (latest) => {
							const latest_managed = latest.documents.get(file_key);
							if (
								latest_managed?.document !== managed.document ||
								EditorFileKeyForFile(outcome.file) !== file_key
							)
								return latest;
							const documents = new Map(latest.documents);
							documents.set(file_key, { ...latest_managed, file: outcome.file });
							return { ...latest, documents };
						});
					}

					return outcome;
				});

			const Close = (file: EditorFileReference) =>
				Effect.gen(function* () {
					const file_key = EditorFileKeyForFile(file);
					const closed = yield* Ref.modify(state, (current) => {
						const managed = current.documents.get(file_key);
						if (managed === undefined) return [undefined, current] as const;
						const documents = new Map(current.documents);
						documents.delete(file_key);
						const is_active =
							Option.getOrUndefined(current.active_file_key) === file_key;
						return [
							{
								is_active,
								open_document: managed,
								surface: Option.getOrUndefined(current.surface),
							},
							{
								...current,
								active_file_key: is_active
									? Option.none()
									: current.active_file_key,
								documents,
							},
						] as const;
					});
					if (closed === undefined) return;
					yield* ReleaseAll([
						...(closed.is_active
							? [
									RunAdapter(() => closed.surface?.set_document(undefined)).pipe(
										Effect.asVoid,
									),
								]
							: []),
						ReleaseDocument(closed.open_document),
					]);
				});

			const Current = Effect.gen(function* () {
				const current = yield* Ref.get(state);
				const document_states = yield* Effect.forEach(
					[...current.documents.entries()],
					([file_key, managed]) =>
						Effect.gen(function* () {
							return [
								file_key,
								(yield* RunAdapter(() => managed.document.get_value())) !==
									managed.file.content,
							] as const;
						}),
				);
				return {
					active_file_key: current.active_file_key,
					dirty_file_keys: new Set(
						document_states
							.filter(([, is_dirty]) => is_dirty)
							.map(([file_key]) => file_key),
					),
					open_file_keys: new Set(current.documents.keys()),
				};
			});

			return EditorService.of({
				Activate,
				Attach,
				Close,
				Current,
				Detach,
				Dispose,
				Mark,
				Save,
				Update,
			});
		}),
	);
