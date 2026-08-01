import {
	autocompletion,
	closeBrackets,
	closeBracketsKeymap,
	completionKeymap,
} from "@codemirror/autocomplete";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import {
	bracketMatching,
	foldGutter,
	foldKeymap,
	indentOnInput,
	indentUnit,
	syntaxHighlighting,
} from "@codemirror/language";
import { type Diagnostic, lintKeymap, linter, setDiagnostics } from "@codemirror/lint";
import { highlightSelectionMatches, searchKeymap } from "@codemirror/search";
import { Compartment, EditorState, type Extension } from "@codemirror/state";
import {
	crosshairCursor,
	drawSelection,
	dropCursor,
	EditorView,
	highlightActiveLine,
	highlightActiveLineGutter,
	highlightSpecialChars,
	keymap,
	lineNumbers,
	rectangularSelection,
} from "@codemirror/view";
import { Effect, Option } from "effect";

import type {
	EditorAdapter,
	EditorDiagnostic,
	EditorDocument,
	EditorSurface,
	EditorSurfaceOptions,
	EditorViewState,
} from "./adapter";
import { EditorLanguageForPath, LoadEditorLanguage } from "./language";
import { artisan_highlight_style, artisan_theme } from "./theme";

/**
 * The CodeMirror 6 implementation of the editor seam.
 *
 * CodeMirror keeps document state inside the view rather than in detached
 * models, so a document that is not on screen holds its own `EditorState` and
 * the view's state is snapshotted back into it whenever it stops being
 * displayed. That snapshot is why a document is a small mutable cell rather
 * than a value.
 *
 * Two compartments per document carry the parts that arrive late: the grammar,
 * which is fetched after first paint, and the surface extensions, which do not
 * exist until a surface displays the document.
 */

interface CodeMirrorDocument extends EditorDocument {
	readonly attach: (view: EditorView) => void;
	readonly detach: () => void;
	readonly diagnostics: () => ReadonlyArray<Diagnostic>;
	readonly disposed: () => boolean;
	readonly language_compartment: Compartment;
	readonly set_diagnostics: (diagnostics: ReadonlyArray<Diagnostic>) => void;
	readonly state: () => EditorState;
	readonly surface_compartment: Compartment;
	readonly take_state: (next: EditorState) => void;
	readonly view: () => EditorView | undefined;
}

interface CodeMirrorViewState {
	readonly anchor: number;
	readonly head: number;
	readonly scroll_top: number;
}

const severity_of: Readonly<Record<EditorDiagnostic["severity"], Diagnostic["severity"]>> = {
	error: "error",
	hint: "hint",
	info: "info",
	warning: "warning",
};

/**
 * Diagnostics arrive as one-based line/column pairs because that is what every
 * provider speaks; CodeMirror addresses the document by absolute offset.
 * Out-of-range positions clamp rather than throw, so a diagnostic that lags one
 * edit behind still lands somewhere sensible.
 */
const offset_of = (state: EditorState, line_number: number, column: number) => {
	const clamped_line = Math.min(Math.max(line_number, 1), state.doc.lines);
	const line = state.doc.line(clamped_line);

	return Math.min(line.from + Math.max(column - 1, 0), line.to);
};

const to_codemirror_diagnostics = (
	state: EditorState,
	diagnostics: ReadonlyArray<EditorDiagnostic>,
): ReadonlyArray<Diagnostic> =>
	diagnostics.map((diagnostic) => {
		const from = offset_of(state, diagnostic.start_line, diagnostic.start_column);
		const to = offset_of(state, diagnostic.end_line, diagnostic.end_column);

		return {
			from,
			message: diagnostic.message,
			severity: severity_of[diagnostic.severity],
			to: Math.max(to, from),
			...(diagnostic.code === undefined ? {} : { source: diagnostic.code }),
		} satisfies Diagnostic;
	});

/**
 * The extensions every displayed document shares. Bracket closing,
 * indent-on-input, folding, and history sit here rather than behind a language
 * server: they read the Lezer tree alone and must work the moment a file opens.
 */
const surface_extensions = (options: EditorSurfaceOptions, document_uri: string): Extension => [
	lineNumbers(),
	highlightActiveLineGutter(),
	highlightSpecialChars(),
	history(),
	foldGutter(),
	drawSelection(),
	dropCursor(),
	EditorState.allowMultipleSelections.of(true),
	indentOnInput(),
	indentUnit.of("\t"),
	syntaxHighlighting(artisan_highlight_style),
	bracketMatching(),
	closeBrackets(),
	autocompletion(),
	rectangularSelection(),
	crosshairCursor(),
	highlightActiveLine(),
	highlightSelectionMatches(),
	/** Markers are pushed by the owning service; nothing lints locally yet. */
	linter(() => []),
	keymap.of([
		...closeBracketsKeymap,
		...defaultKeymap,
		...searchKeymap,
		...historyKeymap,
		...foldKeymap,
		...completionKeymap,
		...lintKeymap,
		indentWithTab,
	]),
	artisan_theme,
	EditorView.updateListener.of((update) => {
		if (!update.docChanged) return;
		options.on_change?.(document_uri, update.state.doc.toString());
	}),
];

/**
 * Grammars resolve after the document is already on screen, so a file paints
 * unhighlighted for a frame instead of waiting on a chunk fetch. The
 * compartment is what allows the swap afterwards without rebuilding state and
 * losing the cursor.
 */
const InstallLanguage = (document: CodeMirrorDocument) =>
	Effect.gen(function* () {
		const language = EditorLanguageForPath(document.uri);
		const loaded = yield* LoadEditorLanguage(language).pipe(Effect.option);
		const support = Option.getOrUndefined(Option.flatten(loaded));
		if (support === undefined || document.disposed()) return;
		const effects = document.language_compartment.reconfigure(support);
		const view = document.view();
		if (view !== undefined) {
			view.dispatch({ effects });
			return;
		}
		document.take_state(document.state().update({ effects }).state);
	});

const make_document = (input: {
	readonly language: string;
	readonly uri: string;
	readonly value: string;
}): CodeMirrorDocument => {
	const language_compartment = new Compartment();
	const surface_compartment = new Compartment();
	let attached: EditorView | undefined = undefined;
	let diagnostics: ReadonlyArray<Diagnostic> = [];
	let is_disposed = false;
	let detached_state = EditorState.create({
		doc: input.value,
		extensions: [language_compartment.of([]), surface_compartment.of([])],
	});

	const state = () => attached?.state ?? detached_state;

	return {
		attach: (view) => {
			attached = view;
		},
		detach: () => {
			if (attached !== undefined) detached_state = attached.state;
			attached = undefined;
		},
		diagnostics: () => diagnostics,
		disposed: () => is_disposed,
		dispose: () => {
			is_disposed = true;
			attached = undefined;
			diagnostics = [];
		},
		get_value: () => state().doc.toString(),
		language_compartment,
		set_diagnostics: (next) => {
			diagnostics = next;
		},
		set_value: (value) => {
			const current = state();
			const transaction = current.update({
				changes: { from: 0, insert: value, to: current.doc.length },
			});
			if (attached !== undefined) {
				attached.dispatch(transaction);
				return;
			}
			detached_state = transaction.state;
		},
		state,
		surface_compartment,
		take_state: (next) => {
			detached_state = next;
		},
		uri: input.uri,
		view: () => attached,
	};
};

/**
 * A surface owns exactly one `EditorView`. Swapping documents reconfigures that
 * view with the incoming document's state instead of constructing a second
 * view, which is what keeps tab switching free of a mount and unmount cycle.
 */
const make_surface = (host: object, options: EditorSurfaceOptions = {}): EditorSurface => {
	const parent = host as HTMLElement;
	let current: CodeMirrorDocument | undefined = undefined;
	const view = new EditorView({ parent, state: EditorState.create({ doc: "" }) });

	const display = (document: CodeMirrorDocument | undefined) => {
		current?.detach();
		current = document;

		if (document === undefined) {
			view.setState(EditorState.create({ doc: "" }));
			return;
		}

		view.setState(document.state());
		view.dispatch({
			effects: document.surface_compartment.reconfigure(
				surface_extensions(options, document.uri),
			),
		});
		document.attach(view);
		if (document.diagnostics().length > 0)
			view.dispatch(setDiagnostics(view.state, [...document.diagnostics()]));
	};

	return {
		dispose: () => {
			current?.detach();
			current = undefined;
			view.destroy();
		},
		get_document: () => current,
		restore_view_state: (state) => {
			const restored = state.opaque as CodeMirrorViewState | undefined;
			if (restored === undefined) return;
			const length = view.state.doc.length;
			view.dispatch({
				selection: {
					anchor: Math.min(restored.anchor, length),
					head: Math.min(restored.head, length),
				},
			});
			view.scrollDOM.scrollTop = restored.scroll_top;
		},
		save_view_state: () => {
			const selection = view.state.selection.main;

			return {
				opaque: {
					anchor: selection.anchor,
					head: selection.head,
					scroll_top: view.scrollDOM.scrollTop,
				} satisfies CodeMirrorViewState,
			} satisfies EditorViewState;
		},
		set_document: (document) => display(document as CodeMirrorDocument | undefined),
	};
};

/** The browser CodeMirror adapter installed by the browser runtime composition. */
export const BrowserCodeMirrorAdapter: EditorAdapter = {
	create_document: make_document,
	create_surface: make_surface,
	install_language: (document) =>
		Effect.gen(function* () {
			yield* InstallLanguage(document as CodeMirrorDocument);
		}),
	set_markers: (document, _owner, diagnostics) => {
		const target = document as CodeMirrorDocument;
		const mapped = to_codemirror_diagnostics(target.state(), diagnostics);
		target.set_diagnostics(mapped);
		const view = target.view();
		if (view !== undefined) view.dispatch(setDiagnostics(view.state, [...mapped]));
	},
};
