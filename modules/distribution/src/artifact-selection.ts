import { Result } from "effect";

import type { SemanticVersion, TargetPlatform } from "./common";
import type { ReleaseArtifact, ReleaseManifest } from "./release-manifest";

export type ArtifactSelectionError =
	| {
			readonly _tag: "BootstrapVersionUnsupported";
			readonly installed_version: SemanticVersion;
			readonly minimum_version: SemanticVersion;
	  }
	| {
			readonly _tag: "CliVersionUnsupported";
			readonly installed_version: SemanticVersion;
			readonly minimum_version: SemanticVersion;
	  }
	| { readonly _tag: "UnsupportedTarget"; readonly target: TargetPlatform }
	| { readonly _tag: "AmbiguousTarget"; readonly target: TargetPlatform };

export interface ArtifactSelectionRequest {
	readonly release: ReleaseManifest;
	readonly target: TargetPlatform;
	readonly bootstrap_version: SemanticVersion;
	readonly cli_version?: SemanticVersion;
}

const parse_semantic_version = (version: SemanticVersion): readonly [number, number, number] => {
	const [major = "0", minor = "0", patch_with_suffix = "0"] = version.split(".");
	const patch = patch_with_suffix.split("-", 1)[0] ?? "0";
	return [Number(major), Number(minor), Number(patch)];
};

export const CompareSemanticVersions = (left: SemanticVersion, right: SemanticVersion): number => {
	const left_parts = parse_semantic_version(left);
	const right_parts = parse_semantic_version(right);

	for (let index = 0; index < left_parts.length; index += 1) {
		const difference = (left_parts[index] ?? 0) - (right_parts[index] ?? 0);
		if (difference !== 0) return difference;
	}
	return 0;
};

const matches_target = (artifact: ReleaseArtifact, target: TargetPlatform): boolean =>
	artifact.platform === target.platform &&
	artifact.architecture === target.architecture &&
	artifact.libc === target.libc;

export const SelectReleaseArtifact = ({
	release,
	target,
	bootstrap_version,
	cli_version,
}: ArtifactSelectionRequest): Result.Result<ReleaseArtifact, ArtifactSelectionError> => {
	if (CompareSemanticVersions(bootstrap_version, release.minimum_installer_version) < 0) {
		return Result.fail({
			_tag: "BootstrapVersionUnsupported",
			installed_version: bootstrap_version,
			minimum_version: release.minimum_installer_version,
		});
	}

	if (
		cli_version !== undefined &&
		CompareSemanticVersions(cli_version, release.minimum_cli_version) < 0
	) {
		return Result.fail({
			_tag: "CliVersionUnsupported",
			installed_version: cli_version,
			minimum_version: release.minimum_cli_version,
		});
	}

	const candidates = release.artifacts.filter((artifact) => matches_target(artifact, target));
	if (candidates.length === 0) return Result.fail({ _tag: "UnsupportedTarget", target });
	if (candidates.length > 1) return Result.fail({ _tag: "AmbiguousTarget", target });
	const [candidate] = candidates;
	return candidate === undefined
		? Result.fail({ _tag: "UnsupportedTarget", target })
		: Result.succeed(candidate);
};
