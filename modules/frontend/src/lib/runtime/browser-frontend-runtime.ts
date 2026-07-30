import { Layer } from "effect";

import { BannerReporterNoopLive, BannerServiceLive } from "../banner/service";
import { SonnerBannerPresenterLive } from "../banner/sonner-presenter";
import { BrowserCodeMirrorAdapter } from "../editor/codemirror-adapter";
import { MakeEditorLayer } from "../editor/service";
import { FrontendRuntimeLive } from "./frontend-runtime";

/**
 * Browser-only composition keeps the editor implementation out of Node-side
 * runtime tests. CodeMirror needs no worker layer of its own — the grammar for
 * a file is fetched on demand by the adapter.
 */
export const BrowserFrontendRuntimeLive = Layer.mergeAll(
	FrontendRuntimeLive,
	BannerServiceLive.pipe(
		Layer.provide(Layer.merge(SonnerBannerPresenterLive, BannerReporterNoopLive)),
	),
	MakeEditorLayer(BrowserCodeMirrorAdapter),
);
