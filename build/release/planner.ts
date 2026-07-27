import { Schema } from "effect";

import {
	candidate_ref,
	FullCommit,
	release_lanes,
	ReleaseEvent,
	ReleaseMode,
	SemanticVersion,
	transport_asset_names,
	type ReleaseEvent as ReleaseEventValue,
	type ReleaseMode as ReleaseModeValue,
} from "./policy.ts";

export type PlanInput = {
	readonly event: ReleaseEventValue;
	readonly ref: string;
	readonly commit: string;
	readonly version: string;
	readonly workspace_version: string;
	readonly run_id: string;
	readonly mode?: ReleaseModeValue;
	readonly resume_commit?: string;
	readonly resume_version?: string;
	readonly resume_run_id?: string;
};

export type ReleasePlan = {
	readonly schema_version: 1;
	readonly event: ReleaseEventValue;
	readonly mode: ReleaseModeValue;
	readonly commit: string;
	readonly version: string;
	readonly run_id: string;
	readonly tag: string;
	readonly candidate_artifact: string;
	readonly construct_candidate: boolean;
	readonly publish: boolean;
	readonly unsigned_prerelease: boolean;
	readonly assets: ReadonlyArray<string>;
	readonly lanes: typeof release_lanes;
};

export const make_release_plan = (input: PlanInput): ReleasePlan => {
	const event = Schema.decodeUnknownSync(ReleaseEvent)(input.event);
	const mode = Schema.decodeUnknownSync(ReleaseMode)(input.mode ?? "dry-run");
	const selected_commit = mode === "resume" ? input.resume_commit : input.commit;
	const selected_version = mode === "resume" ? input.resume_version : input.version;
	const selected_run_id = mode === "resume" ? input.resume_run_id : input.run_id;

	if (!selected_commit || !selected_version || !selected_run_id) {
		throw new Error("Resume requires exact version, commit, and run ID.");
	}
	const commit = Schema.decodeUnknownSync(FullCommit)(selected_commit);
	const version = Schema.decodeUnknownSync(SemanticVersion)(selected_version);
	const workspace_version = Schema.decodeUnknownSync(SemanticVersion)(input.workspace_version);
	if (version !== workspace_version) {
		throw new Error(
			`Requested version ${version} does not match workspace version ${workspace_version}.`,
		);
	}
	if (!/^[1-9]\d*$/.test(selected_run_id)) throw new Error("Run ID must be a positive integer.");
	if (event === "workflow_dispatch" && input.ref !== candidate_ref) {
		throw new Error(
			"Manual candidates and releases must run from the protected candidate branch.",
		);
	}
	if (event !== "workflow_dispatch" && mode !== "dry-run") {
		throw new Error("Push and pull request staging may only use dry-run mode.");
	}

	return Object.freeze({
		schema_version: 1,
		event,
		mode,
		commit,
		version,
		run_id: selected_run_id,
		tag: `v${version}`,
		candidate_artifact: `artisan-candidate-${version}-${commit}-${selected_run_id}`,
		construct_candidate: event === "workflow_dispatch" && mode !== "resume",
		publish: event === "workflow_dispatch" && (mode === "release" || mode === "resume"),
		unsigned_prerelease: true,
		assets: Object.freeze(transport_asset_names(version)),
		lanes: release_lanes,
	});
};
