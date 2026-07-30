/**
 * The editor implementation boundary.
 *
 * Everything above this file — tabs, dirty tracking, save arbitration — is
 * plain Effect state that runs under a fake adapter in tests. Everything below
 * it is browser-only editor code. The service never receives a filesystem, a
 * network client, or an Electron capability; it moves documents and view state
 * between the workspace and whichever adapter is installed.
 */

/** One workspace file as the editor knows it, independent of transport shape. */
export interface EditorWorkspaceFile {
	readonly id: string;
	readonly workspace_id: string;
	readonly path: string;
	readonly language: string;
	readonly revision: string;
	readonly content: string;
}

/** A position-bearing problem to render in the gutter, sourced from any provider. */
export interface EditorDiagnostic {
	readonly code?: string;
	readonly end_column: number;
	readonly end_line: number;
	readonly message: string;
	readonly severity: "error" | "warning" | "info" | "hint";
	readonly start_column: number;
	readonly start_line: number;
}

/** Scroll and selection, opaque so each adapter can carry its own representation. */
export interface EditorViewState {
	readonly opaque: unknown;
}

/**
 * One open document. Adapters that keep document state inside the view (as
 * CodeMirror does) snapshot it back here when the document is deactivated, so
 * a document that is not currently on screen still answers `get_value`.
 */
export interface EditorDocument {
	readonly dispose: () => void;
	readonly get_value: () => string;
	readonly set_value: (value: string) => void;
	readonly uri: string;
}

/** The mounted editing surface. Exactly one exists per attached host element. */
export interface EditorSurface {
	readonly dispose: () => void;
	readonly get_document: () => EditorDocument | undefined;
	readonly restore_view_state: (state: EditorViewState) => void;
	readonly save_view_state: () => EditorViewState | undefined;
	readonly set_document: (document: EditorDocument | undefined) => void;
}

/** Reports edits made in the surface so the owning service can track dirtiness. */
export interface EditorSurfaceOptions {
	readonly on_change?: (uri: string, value: string) => void;
}

/**
 * The one seam every editor implementation fills. Kept deliberately small: a
 * surface, documents, and markers. A second implementation is a new file here,
 * not a change to the service.
 */
export interface EditorAdapter {
	readonly create_document: (input: {
		readonly language: string;
		readonly uri: string;
		readonly value: string;
	}) => EditorDocument;
	readonly create_surface: (host: object, options?: EditorSurfaceOptions) => EditorSurface;
	readonly install_language?: (document: EditorDocument) => Effect.Effect<void>;
	readonly set_markers: (
		document: EditorDocument,
		owner: string,
		diagnostics: ReadonlyArray<EditorDiagnostic>,
	) => void;
}
import type { Effect } from "effect";
