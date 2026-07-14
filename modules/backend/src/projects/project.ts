import { Schema } from "effect";

import { GitRemoteName, IsoDateTime } from "@artisan/protocol";

import {
	GitProviderHost,
	GitProviderId,
	GitProviderUrl,
	GitProviderWebUrl,
} from "../git-provider/git-provider";

const text_encoder = new TextEncoder();

const BoundedVisibleText = Schema.String.check(
	Schema.makeFilter<string>((value) =>
		value.trim().length === 0 ||
		text_encoder.encode(value).byteLength > 512 ||
		/[\p{Cc}\p{Cf}]/u.test(value)
			? "Expected non-empty bounded visible text without control characters"
			: undefined,
	),
);

const BoundedPath = Schema.String.check(
	Schema.makeFilter<string>((value) =>
		value.trim().length === 0 ||
		text_encoder.encode(value).byteLength > 4_096 ||
		/[\p{Cc}\p{Cf}]/u.test(value)
			? "Expected a non-empty bounded path without control characters"
			: undefined,
	),
);

const AccountLogin = Schema.String.check(
	Schema.makeFilter<string>((value) =>
		value.trim().length === 0 ||
		text_encoder.encode(value).byteLength > 128 ||
		/[\p{Cc}\p{Cf}\s]/u.test(value)
			? "Expected a bounded account login without whitespace or control characters"
			: undefined,
	),
);

/** Identifies one hosted project derived from its immutable provider identity. */
export const HostedProjectId = Schema.String.check(
	Schema.isPattern(/^project_[0-9a-f]{64}$/, {
		message: "Expected a stable hosted project ID",
	}),
);

export type HostedProjectId = typeof HostedProjectId.Type;

/** Identifies one workspace derived from its immutable hosted-project identity. */
export const HostedWorkspaceId = Schema.String.check(
	Schema.isPattern(/^workspace_[0-9a-f]{64}$/, {
		message: "Expected a stable hosted workspace ID",
	}),
);

export type HostedWorkspaceId = typeof HostedWorkspaceId.Type;

/** Identifies one verified hosted remote without retaining provider-specific payloads. */
export const ProjectHostedOrigin = Schema.Struct({
	canonical_host: GitProviderHost,
	clone_url: GitProviderUrl,
	fetch_url: GitProviderUrl,
	name: BoundedVisibleText,
	native_id: BoundedVisibleText,
	owner: AccountLogin,
	provider_id: GitProviderId,
	push_url: GitProviderUrl,
	remote_name: GitRemoteName,
	selected_account_login: AccountLogin,
	web_url: GitProviderWebUrl,
});

export type ProjectHostedOrigin = typeof ProjectHostedOrigin.Type;

/** Defines the untrusted hosted-project registration boundary. */
export const RegisterHostedProject = Schema.Struct({
	canonical_root: BoundedPath,
	display_name: BoundedVisibleText,
	hosted_origin: ProjectHostedOrigin,
});

export type RegisterHostedProject = typeof RegisterHostedProject.Type;

/** Represents one durable local project and the hosted identity that owns it. */
export const RegisteredProject = Schema.Struct({
	hosted_origin: ProjectHostedOrigin,
	project: Schema.Struct({
		display_name: BoundedVisibleText,
		project_id: HostedProjectId,
		root_path: BoundedPath,
	}),
	registered_at: IsoDateTime,
	updated_at: IsoDateTime,
	workspace_id: HostedWorkspaceId,
});

export type RegisteredProject = typeof RegisteredProject.Type;

/** Locates one hosted origin by its provider-native identity. */
export const HostedProjectIdentity = Schema.Struct({
	canonical_host: GitProviderHost,
	native_id: BoundedVisibleText,
	provider_id: GitProviderId,
});

export type HostedProjectIdentity = typeof HostedProjectIdentity.Type;

/** Locates one registered project by its stable opaque project ID. */
export const ProjectId = Schema.Struct({ project_id: HostedProjectId });

export type ProjectId = typeof ProjectId.Type;

/** Locates one registered project by its canonical checkout root. */
export const ProjectRoot = Schema.Struct({ canonical_root: BoundedPath });

export type ProjectRoot = typeof ProjectRoot.Type;

/** Locates one registered project by its stable opaque workspace ID. */
export const ProjectWorkspaceId = Schema.Struct({
	workspace_id: HostedWorkspaceId,
});

export type ProjectWorkspaceId = typeof ProjectWorkspaceId.Type;
