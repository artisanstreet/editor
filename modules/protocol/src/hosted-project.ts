import { Schema } from "effect";

import { Identifier, IsoDateTime } from "./common";
import { ProjectRef } from "./thread";

const text_encoder = new TextEncoder();

const BoundedVisibleText = Schema.String.check(
	Schema.makeFilter<string>((value) =>
		value.trim().length === 0 ||
		text_encoder.encode(value).byteLength > 512 ||
		/[\p{Cc}]/u.test(value)
			? "Expected non-empty bounded visible text without control characters"
			: undefined,
	),
);

/** Validates a lowercase bounded hosted Git provider ID. */
export const HostedProjectProviderId = Schema.String.check(
	Schema.isPattern(/^[a-z][a-z0-9_-]{0,63}$/, {
		message: "Expected a lowercase bounded Git provider ID",
	}),
);

export type HostedProjectProviderId = typeof HostedProjectProviderId.Type;

/** Validates a canonical hosted Git provider host. */
export const HostedProjectProviderHost = Schema.String.check(
	Schema.makeFilter<string>((value) =>
		normalize_host(value) === value ? undefined : "Expected a canonical Git provider host",
	),
);

export type HostedProjectProviderHost = typeof HostedProjectProviderHost.Type;

/** Validates an authenticated provider account login without whitespace. */
export const HostedProjectAccountLogin = Schema.String.check(
	Schema.makeFilter<string>((value) =>
		value.trim().length === 0 ||
		text_encoder.encode(value).byteLength > 128 ||
		/[\p{Cc}\p{Cf}\s]/u.test(value)
			? "Expected a bounded account login without whitespace or control characters"
			: undefined,
	),
);

export type HostedProjectAccountLogin = typeof HostedProjectAccountLogin.Type;

/** Validates an absolute native filesystem path without control characters. */
export const HostedProjectNativePath = Schema.String.check(
	Schema.makeFilter<string>((value) =>
		value.length === 0 ||
		text_encoder.encode(value).byteLength > 4_096 ||
		value.includes("\0") ||
		/[\r\n]/u.test(value) ||
		!(/^[A-Za-z]:[\\/]/u.test(value) || value.startsWith("/") || value.startsWith("\\\\"))
			? "Expected a bounded absolute native path without NUL or line breaks"
			: undefined,
	),
);

export type HostedProjectNativePath = typeof HostedProjectNativePath.Type;

/** Validates a bounded clone URL using HTTP(S) or SSH without embedded credentials. */
export const HostedProjectCloneUrl = Schema.String.check(
	Schema.makeFilter<string>((value) =>
		is_provider_url(value, ["http:", "https:", "ssh:"])
			? undefined
			: "Expected a bounded HTTP(S) or SSH URL without credentials or fragments",
	),
);

export type HostedProjectCloneUrl = typeof HostedProjectCloneUrl.Type;

/** Validates a bounded browser URL without credentials or fragments. */
export const HostedProjectWebUrl = Schema.String.check(
	Schema.makeFilter<string>((value) =>
		is_provider_url(value, ["http:", "https:"])
			? undefined
			: "Expected a bounded HTTP(S) URL without credentials or fragments",
	),
);

export type HostedProjectWebUrl = typeof HostedProjectWebUrl.Type;

/** Identifies a repository through its provider-owned immutable identity. */
export const HostedProjectRepositoryOrigin = Schema.Struct({
	native_id: BoundedVisibleText,
	provider_id: HostedProjectProviderId,
	resource_kind: Schema.Literal("repository"),
});

export type HostedProjectRepositoryOrigin = typeof HostedProjectRepositoryOrigin.Type;

/** Identifies a repository by its canonical provider host, owner, and name. */
export const HostedProjectRepositoryIdentity = Schema.Struct({
	host: HostedProjectProviderHost,
	name: BoundedVisibleText,
	owner: HostedProjectAccountLogin,
	provider_id: HostedProjectProviderId,
});

export type HostedProjectRepositoryIdentity = typeof HostedProjectRepositoryIdentity.Type;

/** Represents a known default branch or a provider response that omitted one. */
export const HostedProjectDefaultBranch = Schema.Union([
	Schema.Struct({ _tag: Schema.Literal("known"), name: BoundedVisibleText }),
	Schema.Struct({ _tag: Schema.Literal("unavailable") }),
]);

export type HostedProjectDefaultBranch = typeof HostedProjectDefaultBranch.Type;

/** Projects a repository without retaining provider-specific response data. */
export const HostedProjectRepository = Schema.Struct({
	archived: Schema.Boolean,
	clone_url: HostedProjectCloneUrl,
	default_branch: HostedProjectDefaultBranch,
	identity: HostedProjectRepositoryIdentity,
	origin: HostedProjectRepositoryOrigin,
	viewer_permission: Schema.Literals(["admin", "maintain", "write", "triage", "read", "unknown"]),
	visibility: Schema.Literals(["private", "public", "internal", "unknown"]),
	web_url: HostedProjectWebUrl,
});

export type HostedProjectRepository = typeof HostedProjectRepository.Type;

/** Selects one provider host and account that can clone a repository. */
export const HostedProjectSelection = Schema.Struct({
	account_login: HostedProjectAccountLogin,
	host: HostedProjectProviderHost,
	provider_id: HostedProjectProviderId,
});

export type HostedProjectSelection = typeof HostedProjectSelection.Type;

/** Requests preparation of one hosted repository clone at an approved native path. */
export const HostedProjectCloneRequest = Schema.Struct({
	destination_path: HostedProjectNativePath,
	repository: HostedProjectRepository,
	selection: HostedProjectSelection,
}).check(
	Schema.makeFilter((request) => {
		const repository = request.repository;
		const selection = request.selection;

		return selection.provider_id !== repository.identity.provider_id ||
			selection.provider_id !== repository.origin.provider_id ||
			selection.host !== repository.identity.host ||
			selection.host !== provider_url_host(repository.clone_url) ||
			selection.host !== provider_url_host(repository.web_url)
			? "Expected one provider and host across the selected repository"
			: undefined;
	}),
);

export type HostedProjectCloneRequest = typeof HostedProjectCloneRequest.Type;

/** Projects only the repository facts safe to present in an approval. */
export const HostedProjectApprovalRepository = Schema.Struct({
	host: HostedProjectProviderHost,
	name: BoundedVisibleText,
	owner: HostedProjectAccountLogin,
	provider_id: HostedProjectProviderId,
	selected_account_login: HostedProjectAccountLogin,
	web_url: HostedProjectWebUrl,
}).check(
	Schema.makeFilter((repository) =>
		provider_url_host(repository.web_url) === repository.host
			? undefined
			: "Expected the approval URL to use the repository host",
	),
);

export type HostedProjectApprovalRepository = typeof HostedProjectApprovalRepository.Type;

const HostedProjectCloneApprovalBase = {
	approval_id: Identifier,
	created_at: IsoDateTime,
	destination_path: HostedProjectNativePath,
	repository: HostedProjectApprovalRepository,
	source_command_id: Identifier,
	thread_id: Identifier,
	updated_at: IsoDateTime,
};

const HostedProjectCloneApprovalDecision = {
	decided_at: IsoDateTime,
	decision_message_id: Identifier,
};

const HostedProjectAttachment = Schema.Literals(["attached", "already_attached"]);

/** Projects a hosted clone approval awaiting a user decision. */
export const HostedProjectCloneApprovalRequested = Schema.Struct({
	...HostedProjectCloneApprovalBase,
	state: Schema.Literal("requested"),
});

/** Projects reuse of a project already registered for this hosted repository. */
export const HostedProjectCloneApprovalReused = Schema.Struct({
	...HostedProjectCloneApprovalBase,
	attachment: HostedProjectAttachment,
	project: ProjectRef,
	state: Schema.Literal("reused"),
});

/** Projects an approved clone before it begins execution. */
export const HostedProjectCloneApprovalApproved = Schema.Struct({
	...HostedProjectCloneApprovalBase,
	...HostedProjectCloneApprovalDecision,
	decision: Schema.Literal("approved"),
	state: Schema.Literal("approved"),
});

/** Projects an approved clone while provider execution is in progress. */
export const HostedProjectCloneApprovalExecuting = Schema.Struct({
	...HostedProjectCloneApprovalBase,
	...HostedProjectCloneApprovalDecision,
	decision: Schema.Literal("approved"),
	state: Schema.Literal("executing"),
});

/** Projects a clone and registration that was attached to its thread. */
export const HostedProjectCloneApprovalApplied = Schema.Struct({
	...HostedProjectCloneApprovalBase,
	...HostedProjectCloneApprovalDecision,
	attachment: HostedProjectAttachment,
	decision: Schema.Literal("approved"),
	project: ProjectRef,
	state: Schema.Literal("applied"),
});

/** Projects a clone that registered successfully but could not attach to its thread. */
export const HostedProjectCloneApprovalAttachmentConflict = Schema.Struct({
	...HostedProjectCloneApprovalBase,
	...HostedProjectCloneApprovalDecision,
	decision: Schema.Literal("approved"),
	project: ProjectRef,
	state: Schema.Literal("attachment_conflict"),
});

/** Projects a clone that could not complete after approval. */
export const HostedProjectCloneApprovalRejected = Schema.Struct({
	...HostedProjectCloneApprovalBase,
	...HostedProjectCloneApprovalDecision,
	decision: Schema.Literal("approved"),
	reason: Schema.Literals([
		"destination_unavailable",
		"provider_unavailable",
		"repository_unavailable",
		"thread_unavailable",
	]),
	state: Schema.Literal("rejected"),
});

/** Projects an approved clone whose terminal outcome cannot be verified. */
export const HostedProjectCloneApprovalOutcomeUnknown = Schema.Struct({
	...HostedProjectCloneApprovalBase,
	...HostedProjectCloneApprovalDecision,
	decision: Schema.Literal("approved"),
	reason: Schema.Literals(["interrupted", "verification_failed"]),
	state: Schema.Literal("outcome_unknown"),
});

/** Projects a clone explicitly denied by a user. */
export const HostedProjectCloneApprovalDenied = Schema.Struct({
	...HostedProjectCloneApprovalBase,
	...HostedProjectCloneApprovalDecision,
	decision: Schema.Literal("denied"),
	state: Schema.Literal("denied"),
});

/** Represents every source-free lifecycle state for one hosted clone approval. */
export const HostedProjectCloneApproval = Schema.Union([
	HostedProjectCloneApprovalRequested,
	HostedProjectCloneApprovalReused,
	HostedProjectCloneApprovalApproved,
	HostedProjectCloneApprovalExecuting,
	HostedProjectCloneApprovalApplied,
	HostedProjectCloneApprovalAttachmentConflict,
	HostedProjectCloneApprovalRejected,
	HostedProjectCloneApprovalOutcomeUnknown,
	HostedProjectCloneApprovalDenied,
]);

export type HostedProjectCloneApproval = typeof HostedProjectCloneApproval.Type;

/** Requests one hosted clone approval by its durable identity and thread. */
export const HostedProjectCloneApprovalQuery = Schema.Struct({
	approval_id: Identifier,
	thread_id: Identifier,
});

export type HostedProjectCloneApprovalQuery = typeof HostedProjectCloneApprovalQuery.Type;

/** Returns one source-free hosted clone approval projection. */
export const HostedProjectCloneApprovalQueryResult = Schema.Struct({
	approval: HostedProjectCloneApproval,
});

export type HostedProjectCloneApprovalQueryResult =
	typeof HostedProjectCloneApprovalQueryResult.Type;

/** Records a user decision for one pending hosted clone approval. */
export const HostedProjectCloneApprovalResponseRequest = Schema.Struct({
	approval_id: Identifier,
	approved: Schema.Boolean,
});

export type HostedProjectCloneApprovalResponseRequest =
	typeof HostedProjectCloneApprovalResponseRequest.Type;

/** Announces one source-free hosted clone approval lifecycle update. */
export const HostedProjectCloneApprovalUpdatedEvent = Schema.Struct({
	approval: HostedProjectCloneApproval,
	type: Schema.Literal("hosted.project.clone.approval.updated"),
});

export type HostedProjectCloneApprovalUpdatedEvent =
	typeof HostedProjectCloneApprovalUpdatedEvent.Type;

function normalize_host(input: string): string | undefined {
	const is_host_port = /^(?:\[[0-9A-Fa-f:.]+\]|[^:/?#@\s]+)(?::[0-9]{1,5})?$/.test(input);
	const candidate = `https://${input}`;

	if (
		input.length === 0 ||
		input !== input.trim() ||
		/[\p{Cc}\p{Cf}]/u.test(input) ||
		!is_host_port ||
		!URL.canParse(candidate)
	) {
		return undefined;
	}

	const parsed = URL.parse(candidate);

	if (
		parsed === null ||
		parsed.protocol !== "https:" ||
		parsed.username !== "" ||
		parsed.password !== "" ||
		parsed.pathname !== "/" ||
		parsed.search !== "" ||
		parsed.hash !== "" ||
		parsed.hostname.length === 0
	) {
		return undefined;
	}

	return parsed.port === "" ? parsed.hostname : `${parsed.hostname}:${parsed.port}`;
}

function is_provider_url(value: string, protocols: ReadonlyArray<string>) {
	if (text_encoder.encode(value).byteLength > 2_048 || /[\p{Cc}\p{Cf}]/u.test(value)) {
		return false;
	}

	if (
		protocols.includes("ssh:") &&
		/^[A-Za-z0-9._-]+@[A-Za-z0-9.-]+:[A-Za-z0-9._~/-]+$/u.test(value)
	) {
		return true;
	}

	if (!URL.canParse(value)) {
		return false;
	}

	const parsed = URL.parse(value);

	return (
		parsed !== null &&
		protocols.includes(parsed.protocol) &&
		parsed.hostname.length > 0 &&
		parsed.password === "" &&
		parsed.hash === "" &&
		parsed.search === "" &&
		(parsed.protocol === "ssh:" || parsed.username === "")
	);
}

function provider_url_host(value: string) {
	if (URL.canParse(value)) {
		const parsed = URL.parse(value);

		return parsed === null ? undefined : normalize_host(parsed.host);
	}

	const scp_host = /^[A-Za-z0-9._-]+@(?<host>[A-Za-z0-9.-]+):/u.exec(value)?.groups?.host;

	return scp_host === undefined ? undefined : normalize_host(scp_host);
}
