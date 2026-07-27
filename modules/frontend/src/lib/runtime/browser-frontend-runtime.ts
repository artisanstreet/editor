import { Layer } from "effect";

import { BannerReporterNoopLive, BannerServiceLive } from "../banner/service";
import { SonnerBannerPresenterLive } from "../banner/sonner-presenter";
import { BrowserMonacoAdapter, BrowserMonacoWorkersLive } from "../editor/browser-monaco-adapter";
import { MakeMonacoEditorLayer } from "../editor/monaco-editor-service";
import { FrontendRuntimeLive } from "./frontend-runtime";

/** Browser-only composition keeps Monaco and its workers out of Node-side runtime tests. */
export const BrowserFrontendRuntimeLive = Layer.mergeAll(
	FrontendRuntimeLive,
	BannerServiceLive.pipe(
		Layer.provide(Layer.merge(SonnerBannerPresenterLive, BannerReporterNoopLive)),
	),
	MakeMonacoEditorLayer(BrowserMonacoAdapter),
	BrowserMonacoWorkersLive,
);
