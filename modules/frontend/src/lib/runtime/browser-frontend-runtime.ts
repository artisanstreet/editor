import { Layer } from "effect";

import { BannerReporterNoopLive, BannerServiceLive } from "../banner/service";
import { SonnerBannerPresenterLive } from "../banner/sonner-presenter";
import { RouteNavigationLive } from "../browser/route-navigation-live";
import { BrowserCodeMirrorAdapter } from "../editor/codemirror-adapter";
import { MakeEditorLayer } from "../editor/service";
import { ComposerDraftStoreLive } from "../composer/draft-store";
import { RunUsageControllerLive } from "../context-usage/run-usage-controller";
import { ImageInspectionStoreLive } from "../images/inspection-store";
import { SystemNotificationsLive } from "../notifications/service";
import { WebSystemNotificationPresenterLive } from "../notifications/web-presenter";
import { DraftThreadControllerLive } from "../root/draft-thread";
import { SessionDefaultsControllerLive } from "../settings/session-defaults-controller";
import { FrontendRuntimeLive } from "./frontend-runtime";

/** Controllers consume the one production client supplied by the base runtime. */
const FrontendControllersLive = Layer.mergeAll(
	ComposerDraftStoreLive,
	DraftThreadControllerLive,
	ImageInspectionStoreLive,
	RunUsageControllerLive,
	SessionDefaultsControllerLive,
).pipe(Layer.provide(FrontendRuntimeLive));

/**
 * Browser-only for two reasons at once: the host notification centre is
 * reached through a page API that exists in no Node-side test, and a clicked
 * notification navigates, which needs the routed browser adapter.
 */
const SystemNotificationsRuntimeLive = SystemNotificationsLive.pipe(
	Layer.provide(
		Layer.mergeAll(
			RouteNavigationLive,
			WebSystemNotificationPresenterLive,
			FrontendRuntimeLive,
		),
	),
);

/**
 * Browser-only composition keeps the editor implementation out of Node-side
 * runtime tests. CodeMirror needs no worker layer of its own — the grammar for
 * a file is fetched on demand by the adapter.
 */
export const BrowserFrontendRuntimeLive = Layer.mergeAll(
	FrontendRuntimeLive,
	RouteNavigationLive,
	FrontendControllersLive,
	SystemNotificationsRuntimeLive,
	BannerServiceLive.pipe(
		Layer.provide(Layer.merge(SonnerBannerPresenterLive, BannerReporterNoopLive)),
	),
	MakeEditorLayer(BrowserCodeMirrorAdapter),
);
