import * as monaco from "monaco-editor";
import EditorWorker from "monaco-editor/esm/vs/editor/editor.worker?worker";
import CssWorker from "monaco-editor/esm/vs/language/css/css.worker?worker";
import HtmlWorker from "monaco-editor/esm/vs/language/html/html.worker?worker";
import JsonWorker from "monaco-editor/esm/vs/language/json/json.worker?worker";
import TsWorker from "monaco-editor/esm/vs/language/typescript/ts.worker?worker";

import type {
	MonacoAdapter,
	MonacoDiagnostic,
	MonacoEditor,
	MonacoModel,
	MonacoViewState,
} from "./monaco-editor-service";

let workers_configured = false;

/** Configure ESM workers once; Electron's app protocol supports bundled workers. */
export const ConfigureMonacoWorkers = () => {
	if (workers_configured || typeof self === "undefined") return;

	const monaco_global = self as typeof self & {
		MonacoEnvironment?: {
			getWorker: (module_id: string, label: string) => Worker;
		};
	};
	monaco_global.MonacoEnvironment = {
		getWorker: (_module_id, label) => {
			switch (label) {
				case "css":
				case "less":
				case "scss":
					return new CssWorker();
				case "html":
				case "handlebars":
				case "razor":
					return new HtmlWorker();
				case "json":
					return new JsonWorker();
				case "javascript":
				case "typescript":
					return new TsWorker();
				default:
					return new EditorWorker();
			}
		},
	};
	workers_configured = true;
};

const ToModel = (model: monaco.editor.ITextModel): MonacoModel => ({
	dispose: () => model.dispose(),
	get_value: () => model.getValue(),
	set_value: (value) => model.setValue(value),
	uri: model.uri.toString(),
});

const ToDiagnostic = (diagnostic: MonacoDiagnostic): monaco.editor.IMarkerData => ({
	...(diagnostic.code === undefined ? {} : { code: diagnostic.code }),
	endColumn: diagnostic.end_column,
	endLineNumber: diagnostic.end_line,
	message: diagnostic.message,
	severity:
		diagnostic.severity === "error"
			? monaco.MarkerSeverity.Error
			: diagnostic.severity === "warning"
				? monaco.MarkerSeverity.Warning
				: diagnostic.severity === "info"
					? monaco.MarkerSeverity.Info
					: monaco.MarkerSeverity.Hint,
	startColumn: diagnostic.start_column,
	startLineNumber: diagnostic.start_line,
});

export const BrowserMonacoAdapter: MonacoAdapter = {
	create_editor: (host) => {
		ConfigureMonacoWorkers();
		const editor = monaco.editor.create(host as never, {
			automaticLayout: true,
			fontFamily: "var(--font-mono)",
			fontSize: 12,
			minimap: { enabled: false },
			padding: { bottom: 18, top: 12 },
			scrollBeyondLastLine: false,
			tabSize: 2,
			theme: "vs-dark",
		});

		return {
			dispose: () => editor.dispose(),
			get_model: () => {
				const model = editor.getModel();
				return model === null ? undefined : ToModel(model);
			},
			restore_view_state: (state) =>
				editor.restoreViewState(state.opaque as monaco.editor.ICodeEditorViewState),
			save_view_state: () => {
				const state = editor.saveViewState();
				return state === null ? undefined : ({ opaque: state } satisfies MonacoViewState);
			},
			set_model: (model) =>
				editor.setModel(
					model === undefined
						? null
						: monaco.editor.getModel(monaco.Uri.parse(model.uri)),
				),
		};
	},
	create_model: ({ language, uri, value }) =>
		ToModel(monaco.editor.createModel(value, language, monaco.Uri.parse(uri))),
	set_markers: (model, owner, diagnostics) => {
		const target = monaco.editor.getModel(monaco.Uri.parse(model.uri));
		if (target !== null)
			monaco.editor.setModelMarkers(target, owner, diagnostics.map(ToDiagnostic));
	},
};
