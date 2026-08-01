import { Layer } from "effect";

import { BannerReporterNoopLive, BannerServiceLive } from "../banner/service";
import { SonnerBannerPresenterLive } from "../banner/sonner-presenter";
import { RouteNavigationLive } from "../browser/route-navigation";
import { BrowserCodeMirrorAdapter } from "../editor/codemirror-adapter";
import { MakeEditorLayer } from "../editor/service";
import { RunUsageControllerLive } from "../context-usage/run-usage-controller";
import { DraftThreadControllerLive } from "../root/draft-thread";
import { SessionDefaultsControllerLive } from "../settings/session-defaults-controller";
import { FrontendRuntimeLive } from "./frontend-runtime";

/** Controllers consume the one production client supplied by the base runtime. */
const FrontendControllersLive = Layer.mergeAll(
	DraftThreadControllerLive,
	RunUsageControllerLive,
	SessionDefaultsControllerLive,
).pipe(Layer.provide(FrontendRuntimeLive));

/**
 * Browser-only composition keeps the editor implementation out of Node-side
 * runtime tests. CodeMirror needs no worker layer of its own — the grammar for
 * a file is fetched on demand by the adapter.
 */
export const BrowserFrontendRuntimeLive = Layer.mergeAll(
	FrontendRuntimeLive,
	RouteNavigationLive,
	FrontendControllersLive,
	BannerServiceLive.pipe(
		Layer.provide(Layer.merge(SonnerBannerPresenterLive, BannerReporterNoopLive)),
	),
	MakeEditorLayer(BrowserCodeMirrorAdapter),
);
