import { Effect } from "effect";

import type {
	GitDiffQueryEnvelope,
	GitDiffQueryResultEnvelope,
	GitWorkspaceQueryEnvelope,
	GitWorkspaceQueryResultEnvelope,
	PreviewAssetMetadataQueryEnvelope,
	PreviewAssetMetadataQueryResultEnvelope,
	PreviewTargetGetQueryEnvelope,
	PreviewTargetGetQueryResultEnvelope,
	PreviewTargetListQueryEnvelope,
	PreviewTargetListQueryResultEnvelope,
	ProtocolErrorDetail,
	RichLinkResolveQueryEnvelope,
	RichLinkResolveQueryResultEnvelope,
	WorkspaceChangeDiffQueryEnvelope,
	WorkspaceChangeDiffQueryResultEnvelope,
	WorkspaceChangeListQueryEnvelope,
	WorkspaceChangeListQueryResultEnvelope,
	WorkspaceConflictListQueryEnvelope,
	WorkspaceConflictListQueryResultEnvelope,
	WorkspaceFileDiscoveryQueryEnvelope,
	WorkspaceFileDiscoveryQueryResultEnvelope,
	WorkspaceFileReadQueryEnvelope,
	WorkspaceFileReadQueryResultEnvelope,
	WorkspaceLanguageCapabilitiesQueryEnvelope,
	WorkspaceLanguageCapabilitiesQueryResultEnvelope,
} from "@artisan/protocol";

import { GitService, GitServiceError } from "../../../git/git-service";
import { PreviewCoordinator } from "../../../preview/preview-coordinator";
import { PreviewRepositoryError } from "../../../preview/preview-repository";
import { PreviewRuntimeError } from "../../../preview/preview-runtime";
import { PreviewHealthProbeError } from "../../../preview/preview-target";
import { RuntimeMetadata } from "../../../runtime/runtime-metadata";
import { ToolControlPlane } from "../../../tools/tool-control-plane";
import { WorkspaceChangeRepository } from "../../../workspace/workspace-change-repository";
import {
	WorkspaceChangeDiffService,
	WorkspaceChangeDiffUnavailable,
} from "../../../workspace/workspace-change-diff-service";
import {
	WorkspaceFileService,
	WorkspaceFileServiceError,
} from "../../../workspace/workspace-file-service";

export type WorkspaceInspectionQueryEnvelope =
	| GitDiffQueryEnvelope
	| GitWorkspaceQueryEnvelope
	| PreviewAssetMetadataQueryEnvelope
	| PreviewTargetGetQueryEnvelope
	| PreviewTargetListQueryEnvelope
	| RichLinkResolveQueryEnvelope
	| WorkspaceChangeDiffQueryEnvelope
	| WorkspaceChangeListQueryEnvelope
	| WorkspaceConflictListQueryEnvelope
	| WorkspaceFileDiscoveryQueryEnvelope
	| WorkspaceFileReadQueryEnvelope
	| WorkspaceLanguageCapabilitiesQueryEnvelope;

export type WorkspaceInspectionQueryResultEnvelope =
	| GitDiffQueryResultEnvelope
	| GitWorkspaceQueryResultEnvelope
	| PreviewAssetMetadataQueryResultEnvelope
	| PreviewTargetGetQueryResultEnvelope
	| PreviewTargetListQueryResultEnvelope
	| RichLinkResolveQueryResultEnvelope
	| WorkspaceChangeDiffQueryResultEnvelope
	| WorkspaceChangeListQueryResultEnvelope
	| WorkspaceConflictListQueryResultEnvelope
	| WorkspaceFileDiscoveryQueryResultEnvelope
	| WorkspaceFileReadQueryResultEnvelope
	| WorkspaceLanguageCapabilitiesQueryResultEnvelope;

const WorkspaceErrorDetail = (error: unknown): ProtocolErrorDetail =>
	error instanceof WorkspaceFileServiceError && error.reason === "changed"
		? {
				code: "workspace.conflict",
				message: "The workspace file changed before the requested mutation could apply.",
				retryable: false,
			}
		: {
				code: "workspace.unavailable",
				message: "The workspace operation could not be completed.",
				retryable: true,
			};

const WorkspaceDiffErrorDetail = (error: unknown): ProtocolErrorDetail => {
	if (
		error instanceof WorkspaceChangeDiffUnavailable &&
		(error.reason === "legacy_unavailable" || error.reason === "missing")
	)
		return {
			code: "workspace.diff_unavailable",
			message: "No immutable diff is available for this workspace change.",
			retryable: false,
		};

	return error instanceof WorkspaceChangeDiffUnavailable && error.reason === "erased"
		? {
				code: "workspace.unavailable",
				message: "The workspace change is no longer available.",
				retryable: false,
			}
		: {
				code: "workspace.invariant_failed",
				message: "The immutable workspace diff failed validation.",
				retryable: false,
			};
};

const GitErrorDetail = (error: unknown): ProtocolErrorDetail => {
	if (error instanceof GitServiceError) {
		switch (error.reason) {
			case "busy":
				return {
					code: "git.busy",
					message: "Another Git mutation is already active for this workspace.",
					retryable: true,
				};
			case "changed":
				return {
					code: "git.changed",
					message: "The Git workspace changed; refresh before retrying.",
					retryable: false,
				};
			case "id_conflict":
				return {
					code: "command.id_conflict",
					message: "This command id has already been used for different Git intent.",
					retryable: false,
				};
			case "invalid_path":
				return {
					code: "git.invalid_path",
					message: "One or more paths are not eligible for this Git mutation.",
					retryable: false,
				};
			case "invariant":
				return {
					code: "git.invariant_failed",
					message: "The durable Git state failed validation.",
					retryable: false,
				};
			case "not_repository":
				return {
					code: "git.not_repository",
					message: "The workspace is not a Git repository.",
					retryable: false,
				};
			case "unsupported_state":
				return {
					code: "git.unsupported_state",
					message: "This Git mutation is not supported in the current repository state.",
					retryable: false,
				};
			case "unavailable":
				return {
					code: "git.unavailable",
					message: "The Git operation could not be completed.",
					retryable: error.retryable,
				};
		}
	}

	const tagged = typeof error === "object" && error !== null ? error : undefined;
	const tag =
		tagged !== undefined && "_tag" in tagged && typeof tagged._tag === "string"
			? tagged._tag
			: "";
	const reason =
		tagged !== undefined && "reason" in tagged && typeof tagged.reason === "string"
			? tagged.reason
			: "";

	if (reason === "changed" || reason === "snapshot_changed" || reason === "workspace_changed")
		return {
			code: "git.changed",
			message: "The Git workspace changed; refresh before retrying.",
			retryable: false,
		};
	if (reason === "busy" || reason === "workspace_busy")
		return {
			code: "git.busy",
			message: "Another Git mutation is already active for this workspace.",
			retryable: true,
		};
	if (reason === "not_repository")
		return {
			code: "git.not_repository",
			message: "The workspace is not a Git repository.",
			retryable: false,
		};
	if (
		reason === "not_found" ||
		reason === "thread_unavailable" ||
		reason === "unauthorized" ||
		reason === "workspace_unavailable"
	)
		return {
			code: "git.unavailable",
			message: "The Git workspace is not available.",
			retryable: false,
		};
	if (
		tag.includes("Invariant") ||
		tag.includes("Invalid") ||
		reason === "corrupt" ||
		reason === "invariant"
	)
		return {
			code: "git.invariant_failed",
			message: "The durable Git state failed validation.",
			retryable: false,
		};

	return {
		code: "git.unavailable",
		message: "The Git operation could not be completed.",
		retryable: true,
	};
};

const PreviewErrorDetail = (error: unknown): ProtocolErrorDetail => {
	if (error instanceof PreviewRepositoryError)
		return error.code === "invalid"
			? {
					code: "preview.invalid",
					message: "The preview request conflicts with the durable preview state.",
					retryable: false,
				}
			: error.code === "not_found"
				? {
						code: "preview.not_found",
						message:
							"The requested preview target or inspection session is unavailable.",
						retryable: false,
					}
				: {
						code: "preview.storage_unavailable",
						message: "The preview state could not be durably read or updated.",
						retryable: true,
					};
	if (error instanceof PreviewHealthProbeError)
		return {
			code: "preview.health_unavailable",
			message: "The local preview health probe is currently unavailable.",
			retryable: true,
		};
	if (error instanceof PreviewRuntimeError)
		return error.code === "invalid_input" || error.code === "not_found"
			? {
					code: error.code === "invalid_input" ? "preview.invalid" : "preview.not_found",
					message: "The requested preview runtime resource is unavailable.",
					retryable: false,
				}
			: error.code === "browser_unavailable"
				? {
						code: "preview.browser_unavailable",
						message: "The external browser opener is currently unavailable.",
						retryable: true,
					}
				: {
						code: "preview.connector_unavailable",
						message: "The external preview connector is currently unavailable.",
						retryable: true,
					};
	return {
		code: "preview.unavailable",
		message: "The preview operation could not be completed.",
		retryable: true,
	};
};

export const MakeWorkspaceInspectionQueryHandler = Effect.gen(function* () {
	const git = yield* GitService;
	const metadata = yield* RuntimeMetadata;
	const previews = yield* PreviewCoordinator;
	const tools = yield* ToolControlPlane;
	const workspace_changes = yield* WorkspaceChangeRepository;
	const workspace_diffs = yield* WorkspaceChangeDiffService;
	const workspace_files = yield* WorkspaceFileService;

	const Envelope = <Kind extends WorkspaceInspectionQueryResultEnvelope["kind"], Payload>(
		query: WorkspaceInspectionQueryEnvelope,
		kind: Kind,
		payload: Payload,
	) =>
		Effect.gen(function* () {
			return {
				correlation_id: query.message_id,
				kind,
				message_id: yield* metadata.MakeId("message"),
				origin: "backend" as const,
				payload,
				protocol_version: 1 as const,
				schema_version: 1 as const,
				sent_at: yield* metadata.Now,
			};
		});

	const handlers = {
		"workspace.file.read.query": (query: WorkspaceFileReadQueryEnvelope) =>
			workspace_files.Read(query.payload).pipe(
				Effect.flatMap((payload) =>
					Envelope(query, "workspace.file.read.query.result", payload),
				),
				Effect.mapError(WorkspaceErrorDetail),
			),
		"workspace.change.list.query": (query: WorkspaceChangeListQueryEnvelope) =>
			workspace_changes.List(query.payload.thread_id, query.payload.workspace_id).pipe(
				Effect.flatMap((payload) =>
					Envelope(query, "workspace.change.list.query.result", payload),
				),
				Effect.mapError(
					() =>
						({
							code: "projection.unavailable",
							message: "The workspace change projection could not be read.",
							retryable: true,
						}) satisfies ProtocolErrorDetail,
				),
			),
		"workspace.conflict.list.query": (query: WorkspaceConflictListQueryEnvelope) =>
			workspace_changes.ListConflictSnapshot(query.payload.thread_id).pipe(
				Effect.flatMap((payload) =>
					Envelope(query, "workspace.conflict.list.query.result", payload),
				),
				Effect.mapError(
					() =>
						({
							code: "projection.unavailable",
							message: "The workspace conflict projection could not be read.",
							retryable: true,
						}) satisfies ProtocolErrorDetail,
				),
			),
		"workspace.change.diff.query": (query: WorkspaceChangeDiffQueryEnvelope) =>
			workspace_diffs.Read(query.payload).pipe(
				Effect.flatMap((payload) =>
					Envelope(query, "workspace.change.diff.query.result", payload),
				),
				Effect.mapError(WorkspaceDiffErrorDetail),
			),
		"git.workspace.query": (query: GitWorkspaceQueryEnvelope) =>
			git.Query(query).pipe(
				Effect.flatMap((payload) => Envelope(query, "git.workspace.query.result", payload)),
				Effect.mapError(GitErrorDetail),
			),
		"git.diff.query": (query: GitDiffQueryEnvelope) =>
			git.Diff(query).pipe(
				Effect.flatMap((payload) => Envelope(query, "git.diff.query.result", payload)),
				Effect.mapError(GitErrorDetail),
			),
		"workspace.file.discovery.query": (query: WorkspaceFileDiscoveryQueryEnvelope) =>
			tools.Discover(query.payload).pipe(
				Effect.flatMap((payload) =>
					Envelope(query, "workspace.file.discovery.query.result", payload),
				),
				Effect.mapError(
					() =>
						({
							code: "workspace.discovery.unavailable",
							message: "Workspace file discovery is unavailable.",
							retryable: true,
						}) satisfies ProtocolErrorDetail,
				),
			),
		"workspace.language.capabilities.query": (
			query: WorkspaceLanguageCapabilitiesQueryEnvelope,
		) =>
			tools.LanguageCapabilities(query.payload).pipe(
				Effect.flatMap((payload) =>
					Envelope(query, "workspace.language.capabilities.query.result", payload),
				),
				Effect.mapError(
					() =>
						({
							code: "workspace.language.unavailable",
							message: "Language capabilities are unavailable.",
							retryable: true,
						}) satisfies ProtocolErrorDetail,
				),
			),
		"preview.target.list.query": (query: PreviewTargetListQueryEnvelope) =>
			previews.List(query.payload.workspace_id).pipe(
				Effect.flatMap((targets) =>
					Envelope(query, "preview.target.list.query.result", { targets }),
				),
				Effect.mapError(PreviewErrorDetail),
			),
		"preview.target.get.query": (query: PreviewTargetGetQueryEnvelope) =>
			previews.Get(query.payload.target_id).pipe(
				Effect.flatMap((payload) =>
					Envelope(query, "preview.target.get.query.result", payload),
				),
				Effect.mapError(PreviewErrorDetail),
			),
		"preview.rich_link.resolve.query": (query: RichLinkResolveQueryEnvelope) =>
			previews.ResolveRichLink(query.payload.url).pipe(
				Effect.flatMap((payload) =>
					Envelope(query, "preview.rich_link.resolve.query.result", payload),
				),
				Effect.mapError(PreviewErrorDetail),
			),
		"preview.asset.metadata.query": (query: PreviewAssetMetadataQueryEnvelope) =>
			previews.AssetMetadata(query.payload.asset_id).pipe(
				Effect.flatMap((asset) =>
					asset === undefined
						? Effect.fail(
								new PreviewRepositoryError({
									code: "not_found",
									message: "Preview asset not found",
								}),
							)
						: Envelope(query, "preview.asset.metadata.query.result", asset),
				),
				Effect.mapError(PreviewErrorDetail),
			),
	};

	return (
		query: WorkspaceInspectionQueryEnvelope,
	): Effect.Effect<WorkspaceInspectionQueryResultEnvelope, ProtocolErrorDetail> => {
		switch (query.kind) {
			case "workspace.file.read.query":
				return handlers["workspace.file.read.query"](query);
			case "workspace.change.list.query":
				return handlers["workspace.change.list.query"](query);
			case "workspace.conflict.list.query":
				return handlers["workspace.conflict.list.query"](query);
			case "workspace.change.diff.query":
				return handlers["workspace.change.diff.query"](query);
			case "git.workspace.query":
				return handlers["git.workspace.query"](query);
			case "git.diff.query":
				return handlers["git.diff.query"](query);
			case "workspace.file.discovery.query":
				return handlers["workspace.file.discovery.query"](query);
			case "workspace.language.capabilities.query":
				return handlers["workspace.language.capabilities.query"](query);
			case "preview.target.list.query":
				return handlers["preview.target.list.query"](query);
			case "preview.target.get.query":
				return handlers["preview.target.get.query"](query);
			case "preview.rich_link.resolve.query":
				return handlers["preview.rich_link.resolve.query"](query);
			case "preview.asset.metadata.query":
				return handlers["preview.asset.metadata.query"](query);
		}
	};
});
