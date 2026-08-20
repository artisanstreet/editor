import { Layer } from "effect";

import { RouteNavigationLive } from "../browser/route-navigation-live";
import { BrowserReaderAttentionLive } from "../browser/reader-attention";
import { BrowserTypographyLive } from "../browser/typography";
import { MathRendererControllerLive } from "../components/markdown/math-renderer-controller";
import { RichLinkAssetControllerLive } from "../components/markdown/rich-link-asset-controller";
import { RichLinkMetadataControllerLive } from "../components/markdown/rich-link-metadata-controller";
import { BrowserCodeMirrorAdapter } from "../editor/codemirror-adapter";
import { MakeEditorLayer } from "../editor/service";
import { ComposerDraftStoreLive } from "../composer/draft-store";
import { ThreadChecklistLive } from "../conversation/checklist";
import { ThreadOrchestrationRosterLive } from "../orchestration/service";
import { RunUsageControllerLive } from "../context-usage/run-usage-controller";
import { HostIdentityControllerLive } from "../identity/host-identity-controller";
import { HostMachinesControllerLive } from "../identity/host-machines-controller";
import { EngineUsageControllerLive } from "../identity/engine-usage-controller";
import { ImageInspectionStoreLive } from "../images/inspection-store";
import { SystemNotificationsLive } from "../notifications/service";
import { WebSystemNotificationPresenterLive } from "../notifications/web-presenter";
import { DraftThreadControllerLive } from "../root/draft-thread";
import { WorkspaceCatalogControllerLive } from "../root/workspace-catalog-controller";
import { EngineInstallationsControllerLive } from "../settings/engine-installations-controller";
import { SessionDefaultsControllerLive } from "../settings/session-defaults-controller";
import { ThreadRetentionPolicyControllerLive } from "../settings/thread-retention-policy-controller";
import { ThreadTerminalsControllerLive } from "../terminal/thread-terminals-controller";
import { ThreadOpenControllerLive } from "../thread-interaction/thread-open-controller";
import { ThreadSessionProjectionLive } from "../thread-interaction/session-projection";
import { ProjectRepositoryControllerLive } from "../workspace/project-repository-controller";
import { GitWorkspaceControllerLive } from "../workspace/git-workspace-controller";
import { AttentionReconnectLive } from "./attention-reconnect";
import { FrontendRuntimeLive } from "./frontend-runtime";
import { HostResumeRecoveryLive } from "./host-resume-recovery";

/** Controllers consume the one production client supplied by the base runtime. */
const FrontendControllersLive = Layer.mergeAll(
	ComposerDraftStoreLive,
	MathRendererControllerLive,
	RichLinkAssetControllerLive,
	RichLinkMetadataControllerLive,
	DraftThreadControllerLive,
	WorkspaceCatalogControllerLive,
	ImageInspectionStoreLive,
	RunUsageControllerLive,
	HostIdentityControllerLive,
	HostMachinesControllerLive,
	EngineUsageControllerLive,
	EngineInstallationsControllerLive,
	SessionDefaultsControllerLive,
	ThreadRetentionPolicyControllerLive,
	ThreadTerminalsControllerLive,
	ThreadOpenControllerLive,
	ThreadSessionProjectionLive,
	ThreadChecklistLive,
	ThreadOrchestrationRosterLive,
	ProjectRepositoryControllerLive,
	GitWorkspaceControllerLive,
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
 * The transport intentionally has a finite reconnect budget. A renderer that
 * slept through that budget gets exactly one fresh attempt when wall time and
 * the monotonic scheduler disagree after resume.
 */
const HostResumeRecoveryRuntimeLive = HostResumeRecoveryLive.pipe(
	Layer.provide(FrontendRuntimeLive),
);

/**
 * A budget spent while the window was hidden must not strand the gate until
 * a manual reload; returning attention re-arms the same bounded retry.
 */
const AttentionReconnectRuntimeLive = AttentionReconnectLive.pipe(
	Layer.provide(Layer.mergeAll(BrowserReaderAttentionLive, FrontendRuntimeLive)),
);

/**
 * Browser-only composition keeps the editor implementation out of Node-side
 * runtime tests. CodeMirror needs no worker layer of its own — the grammar for
 * a file is fetched on demand by the adapter.
 */
export const BrowserFrontendRuntimeLive = Layer.mergeAll(
	FrontendRuntimeLive,
	HostResumeRecoveryRuntimeLive,
	AttentionReconnectRuntimeLive,
	BrowserReaderAttentionLive,
	BrowserTypographyLive,
	RouteNavigationLive,
	FrontendControllersLive,
	SystemNotificationsRuntimeLive,
	MakeEditorLayer(BrowserCodeMirrorAdapter),
);
