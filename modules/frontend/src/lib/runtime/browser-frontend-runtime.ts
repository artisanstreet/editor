import { Layer } from "effect";

import { BrowserMonacoAdapter } from "../editor/browser-monaco-adapter";
import { MakeMonacoEditorLayer } from "../editor/monaco-editor-service";
import { FrontendRuntimeLive } from "./frontend-runtime";

/** Browser-only composition keeps Monaco and its workers out of Node-side runtime tests. */
export const BrowserFrontendRuntimeLive = Layer.merge(
	FrontendRuntimeLive,
	MakeMonacoEditorLayer(BrowserMonacoAdapter),
);
