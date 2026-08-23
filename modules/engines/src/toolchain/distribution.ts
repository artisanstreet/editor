import { Data, Effect, Schema } from "effect";

import { claude_native_continuation_version } from "../claude/probe";
import { CodexTransportMetadata } from "../codex/protocol";
import { ToolchainReleaseHttp, type ToolchainHttpFailure } from "./http";

const maximum_metadata_bytes = 4 * 1024 * 1024;

/** Upper bound for one engine binary; both vendors ship well under this today. */
export const maximum_engine_binary_bytes = 1024 * 1024 * 1024;

const SemanticVersionPattern = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/;

export const ToolchainSemanticVersion = Schema.String.check(
	Schema.isPattern(SemanticVersionPattern),
);

/** Orders strict SemVer versions, including prerelease precedence (SemVer 2.0). */
export const compare_toolchain_versions = (left: string, right: string) => {
	const parse = (value: string) => {
		const [release, prerelease] = value.split("-", 2);
		return {
			parts: release!.split(".").map((part) => Number.parseInt(part, 10)),
			prerelease: prerelease?.split(".") ?? [],
		};
	};
	const left_version = parse(left);
	const right_version = parse(right);
	for (let index = 0; index < 3; index += 1) {
		const difference = (left_version.parts[index] ?? 0) - (right_version.parts[index] ?? 0);
		if (difference !== 0) return difference;
	}
	if (left_version.prerelease.length === 0 || right_version.prerelease.length === 0)
		return left_version.prerelease.length === 0 ? 1 : -1;
	for (
		let index = 0;
		index < Math.max(left_version.prerelease.length, right_version.prerelease.length);
		index += 1
	) {
		const left_identifier = left_version.prerelease[index];
		const right_identifier = right_version.prerelease[index];
		if (left_identifier === undefined) return -1;
		if (right_identifier === undefined) return 1;
		if (left_identifier === right_identifier) continue;
		const left_numeric = /^\d+$/.test(left_identifier);
		const right_numeric = /^\d+$/.test(right_identifier);
		if (left_numeric && right_numeric)
			return Number.parseInt(left_identifier, 10) - Number.parseInt(right_identifier, 10);
		if (left_numeric) return -1;
		if (right_numeric) return 1;
		return left_identifier.localeCompare(right_identifier);
	}
	return 0;
};

export interface ToolchainPlatformTarget {
	readonly architecture: string;
	readonly platform: NodeJS.Platform;
}

/** One concrete raw executable with its integrity expectations. */
export interface ResolvedBinaryEngineRelease {
	readonly artifact_kind?: "binary";
	readonly binary: string;
	readonly sha256: string;
	readonly size_bytes?: number;
	readonly url: string;
	readonly version: string;
}

/** One NPM platform tarball whose single expected executable is extracted without scripts. */
export interface ResolvedNpmTarballEngineRelease {
	readonly archive_member: string;
	readonly artifact_kind: "npm-tarball";
	readonly binary: string;
	readonly integrity_sha512: string;
	readonly size_bytes?: number;
	readonly url: string;
	readonly version: string;
}

/** One ZIP bundle whose root is stripped into an immutable generation directory. */
export interface ResolvedZipBundleEngineRelease {
	readonly archive_kind: "zip";
	readonly archive_root: string;
	readonly artifact_kind: "archive-bundle";
	/** Launcher basename after archive_root is stripped. */
	readonly binary: string;
	readonly expanded_size_bytes: number;
	readonly sha256: string;
	readonly size_bytes?: number;
	readonly url: string;
	readonly version: string;
}

/** A checksum-pinned vendor installer run stage-by-stage into one owned generation. */
export interface ResolvedStagedInstallerEngineRelease {
	readonly artifact_kind: "staged-installer";
	/** Executable path relative to the immutable generation directory. */
	readonly binary: string;
	readonly commit: string;
	readonly installer_sha256: string;
	readonly size_bytes?: number;
	readonly stages: ReadonlyArray<string>;
	readonly url: string;
	readonly version: string;
}

export type ResolvedEngineRelease =
	| ResolvedBinaryEngineRelease
	| ResolvedNpmTarballEngineRelease
	| ResolvedZipBundleEngineRelease
	| ResolvedStagedInstallerEngineRelease;

export class ToolchainReleaseError extends Data.TaggedError("ToolchainReleaseError")<{
	readonly cause?: unknown;
	readonly engine_id: string;
	readonly reason: "manifest" | "platform_unsupported" | "version_unavailable";
}> {}

export type EngineDistributionFailure = ToolchainHttpFailure | ToolchainReleaseError;

/**
 * Describes how Artisan acquires and owns one engine's binaries: where its
 * releases live, which version policy applies, and which vendor state seeds
 * the owned config home. This is the single home for per-engine distribution
 * policy — the continuation gates the adapters enforce are pinned here as the
 * recommended (tested) versions.
 */
export interface EngineDistribution {
	/** API-authenticated engines reject the generic interactive CLI login path. */
	readonly authentication?: "cli" | "service-api";
	/** File names copied from the vendor's own home into the owned home once. */
	readonly credential_files: ReadonlyArray<string>;
	readonly display_name: string;
	readonly engine_id: string;
	/** The environment variable that redirects the engine to its owned home. */
	readonly home_environment_variable: string;
	/** Vendor-owned interactive browser login invocation for the managed binary. */
	readonly login_args: ReadonlyArray<string>;
	readonly LatestVersion: Effect.Effect<string, EngineDistributionFailure, ToolchainReleaseHttp>;
	readonly minimum_version?: string;
	readonly recommended_version?: string;
	readonly ResolveRelease: (
		version: string,
		target: ToolchainPlatformTarget,
	) => Effect.Effect<ResolvedEngineRelease, EngineDistributionFailure, ToolchainReleaseHttp>;
	/** The vendor home directory name under the user profile, e.g. `.claude`. */
	readonly vendor_home_directory: string;
}

const claude_release_base =
	"https://storage.googleapis.com/claude-code-dist-86c565f3-f756-42ad-8dfa-d59b1c096819/claude-code-releases";

const ClaudeReleaseManifest = Schema.Struct({
	platforms: Schema.Record(
		Schema.String,
		Schema.Struct({
			binary: Schema.optional(Schema.String),
			checksum: Schema.String,
			size: Schema.optional(Schema.Number),
		}),
	),
	version: Schema.String,
});

const claude_platform_key = (target: ToolchainPlatformTarget) => {
	const platform =
		target.platform === "win32" || target.platform === "darwin" || target.platform === "linux"
			? target.platform
			: undefined;
	if (platform === undefined) return undefined;
	return `${platform}-${target.architecture === "arm64" ? "arm64" : "x64"}`;
};

const DecodeVersionText = (engine_id: string, text: string) =>
	Schema.decodeUnknownEffect(ToolchainSemanticVersion)(text.trim().replace(/^"|"$/g, "")).pipe(
		Effect.mapError(
			(cause) => new ToolchainReleaseError({ cause, engine_id, reason: "manifest" }),
		),
	);

const ClaudeDistribution: EngineDistribution = {
	credential_files: [".credentials.json"],
	display_name: "Claude Code",
	engine_id: "claude",
	home_environment_variable: "CLAUDE_CONFIG_DIR",
	login_args: ["auth", "login"],
	LatestVersion: Effect.gen(function* () {
		const http = yield* ToolchainReleaseHttp;
		const stable = yield* http.Get(`${claude_release_base}/stable`, maximum_metadata_bytes);
		return yield* DecodeVersionText("claude", new TextDecoder().decode(stable.bytes));
	}),
	recommended_version: claude_native_continuation_version,
	ResolveRelease: (version, target) =>
		Effect.gen(function* () {
			const http = yield* ToolchainReleaseHttp;
			const platform_key = claude_platform_key(target);
			if (platform_key === undefined)
				return yield* new ToolchainReleaseError({
					engine_id: "claude",
					reason: "platform_unsupported",
				});
			const manifest_bytes = yield* http
				.Get(`${claude_release_base}/${version}/manifest.json`, maximum_metadata_bytes)
				.pipe(
					Effect.mapError(
						(cause) =>
							new ToolchainReleaseError({
								cause,
								engine_id: "claude",
								reason: "version_unavailable",
							}),
					),
				);
			const manifest = yield* Schema.decodeUnknownEffect(
				Schema.fromJsonString(ClaudeReleaseManifest),
			)(new TextDecoder().decode(manifest_bytes.bytes)).pipe(
				Effect.mapError(
					(cause) =>
						new ToolchainReleaseError({
							cause,
							engine_id: "claude",
							reason: "manifest",
						}),
				),
			);
			const platform_release = manifest.platforms[platform_key];
			if (platform_release === undefined)
				return yield* new ToolchainReleaseError({
					engine_id: "claude",
					reason: "platform_unsupported",
				});
			const binary =
				platform_release.binary ?? (target.platform === "win32" ? "claude.exe" : "claude");
			return {
				binary,
				sha256: platform_release.checksum.toLowerCase(),
				...(platform_release.size === undefined
					? {}
					: { size_bytes: platform_release.size }),
				url: `${claude_release_base}/${version}/${platform_key}/${binary}`,
				version,
			} satisfies ResolvedEngineRelease;
		}),
	vendor_home_directory: ".claude",
};

const codex_release_api = "https://api.github.com/repos/openai/codex/releases";

const CodexRelease = Schema.Struct({
	assets: Schema.Array(
		Schema.Struct({
			browser_download_url: Schema.String,
			digest: Schema.optional(Schema.NullOr(Schema.String)),
			name: Schema.String,
			size: Schema.optional(Schema.Number),
		}),
	),
	tag_name: Schema.String,
});

const codex_windows_asset = (architecture: string) =>
	`codex-${architecture === "arm64" ? "aarch64" : "x86_64"}-pc-windows-msvc.exe`;

const CodexDistribution: EngineDistribution = {
	credential_files: ["auth.json"],
	display_name: "Codex",
	engine_id: "codex",
	home_environment_variable: "CODEX_HOME",
	login_args: ["login"],
	LatestVersion: Effect.gen(function* () {
		const http = yield* ToolchainReleaseHttp;
		const latest = yield* http.Get(`${codex_release_api}/latest`, maximum_metadata_bytes);
		const release = yield* Schema.decodeUnknownEffect(Schema.fromJsonString(CodexRelease))(
			new TextDecoder().decode(latest.bytes),
		).pipe(
			Effect.mapError(
				(cause) =>
					new ToolchainReleaseError({ cause, engine_id: "codex", reason: "manifest" }),
			),
		);
		return yield* DecodeVersionText("codex", release.tag_name.replace(/^rust-v/, ""));
	}),
	minimum_version: CodexTransportMetadata.minimum_cli_version,
	recommended_version: CodexTransportMetadata.continuation_cli_version,
	ResolveRelease: (version, target) =>
		Effect.gen(function* () {
			/**
			 * Codex publishes raw executables only for Windows; POSIX releases are
			 * archives whose extraction the toolchain does not carry yet.
			 */
			if (target.platform !== "win32")
				return yield* new ToolchainReleaseError({
					engine_id: "codex",
					reason: "platform_unsupported",
				});
			const http = yield* ToolchainReleaseHttp;
			const tagged = yield* http
				.Get(`${codex_release_api}/tags/rust-v${version}`, maximum_metadata_bytes)
				.pipe(
					Effect.mapError(
						(cause) =>
							new ToolchainReleaseError({
								cause,
								engine_id: "codex",
								reason: "version_unavailable",
							}),
					),
				);
			const release = yield* Schema.decodeUnknownEffect(Schema.fromJsonString(CodexRelease))(
				new TextDecoder().decode(tagged.bytes),
			).pipe(
				Effect.mapError(
					(cause) =>
						new ToolchainReleaseError({
							cause,
							engine_id: "codex",
							reason: "manifest",
						}),
				),
			);
			const asset_name = codex_windows_asset(target.architecture);
			const asset = release.assets.find((candidate) => candidate.name === asset_name);
			const digest = asset?.digest ?? undefined;
			if (asset === undefined || digest === undefined || !digest.startsWith("sha256:"))
				return yield* new ToolchainReleaseError({
					engine_id: "codex",
					reason: "manifest",
				});
			return {
				binary: "codex.exe",
				sha256: digest.slice("sha256:".length).toLowerCase(),
				...(asset.size === undefined ? {} : { size_bytes: asset.size }),
				url: asset.browser_download_url,
				version,
			} satisfies ResolvedEngineRelease;
		}),
	vendor_home_directory: ".codex",
};

/**
 * Grok does not publish detached checksums for its mutable stable channel, so
 * Artisan deliberately pins the vendor binary it has reviewed instead of
 * treating the channel pointer itself as an integrity statement.
 */
export const grok_certified_version = "1.0.5";
const grok_certified_windows_x64_sha256 =
	"4b924daa801663ea20e96382408b1f2b5ba39efad62c14d20d88618a9eb0be64";
const grok_release_base = "https://x.ai/cli";

const GrokDistribution: EngineDistribution = {
	credential_files: ["auth.json"],
	display_name: "Grok Build",
	engine_id: "grok",
	home_environment_variable: "GROK_HOME",
	login_args: ["login"],
	LatestVersion: Effect.gen(function* () {
		const http = yield* ToolchainReleaseHttp;
		const stable = yield* http.Get(`${grok_release_base}/stable`, maximum_metadata_bytes);
		return yield* DecodeVersionText("grok", new TextDecoder().decode(stable.bytes));
	}),
	recommended_version: grok_certified_version,
	ResolveRelease: (version, target) =>
		Effect.gen(function* () {
			if (version !== grok_certified_version)
				return yield* new ToolchainReleaseError({
					engine_id: "grok",
					reason: "version_unavailable",
				});
			if (target.platform !== "win32" || target.architecture !== "x64")
				return yield* new ToolchainReleaseError({
					engine_id: "grok",
					reason: "platform_unsupported",
				});
			return {
				binary: "grok.exe",
				sha256: grok_certified_windows_x64_sha256,
				size_bytes: 142_651_720,
				url: `${grok_release_base}/grok-${version}-windows-x86_64.exe`,
				version,
			} satisfies ResolvedBinaryEngineRelease;
		}),
	vendor_home_directory: ".grok",
};

/** Cursor's installer points at a complete Node/native bundle, not one executable. */
export const cursor_certified_version = "2026.08.11-e8db854";
const cursor_certified_windows_x64_sha256 =
	"0458981ffe0fda840d19b97d7cbcb26832dafcf01a9c229f3fb0e0d233d66c4b";
const cursor_release_base = `https://downloads.cursor.com/lab/${cursor_certified_version}`;
const cursor_installer_url = "https://cursor.com/install?win32=true";

const CursorDistribution: EngineDistribution = {
	credential_files: ["auth.json"],
	display_name: "Cursor",
	engine_id: "cursor",
	home_environment_variable: "CURSOR_CONFIG_DIR",
	login_args: ["login"],
	LatestVersion: Effect.gen(function* () {
		const http = yield* ToolchainReleaseHttp;
		const installer = yield* http.Get(cursor_installer_url, maximum_metadata_bytes);
		const source = new TextDecoder("utf-8", { fatal: true }).decode(installer.bytes);
		const version = /\$version\s*=\s*'([^']+)'/.exec(source)?.[1];
		if (version === undefined)
			return yield* new ToolchainReleaseError({
				engine_id: "cursor",
				reason: "manifest",
			});
		return yield* DecodeVersionText("cursor", version);
	}),
	recommended_version: cursor_certified_version,
	ResolveRelease: (version, target) =>
		Effect.gen(function* () {
			if (version !== cursor_certified_version)
				return yield* new ToolchainReleaseError({
					engine_id: "cursor",
					reason: "version_unavailable",
				});
			if (target.platform !== "win32" || target.architecture !== "x64")
				return yield* new ToolchainReleaseError({
					engine_id: "cursor",
					reason: "platform_unsupported",
				});
			return {
				archive_kind: "zip",
				archive_root: "dist-package",
				artifact_kind: "archive-bundle",
				binary: "cursor-agent.cmd",
				expanded_size_bytes: 384 * 1024 * 1024,
				sha256: cursor_certified_windows_x64_sha256,
				size_bytes: 73_841_982,
				url: `${cursor_release_base}/windows/x64/agent-cli-package.zip`,
				version,
			} satisfies ResolvedZipBundleEngineRelease;
		}),
	vendor_home_directory: ".cursor",
};

/**
 * OpenCode is a separate beta channel and executable. The pin is deliberate:
 * V2's API/config/plugin contracts are experimental, so upgrades require a
 * compatibility review instead of following an NPM dist-tag at runtime.
 */
export const opencode2_certified_version = "0.0.0-beta-17778";
export const opencode2_certified_upstream_commit = "0d2684b67308380fc47540fe55deb55306a08e3f";

const OpenCode2Distribution: EngineDistribution = {
	authentication: "service-api",
	credential_files: [],
	display_name: "OpenCode",
	engine_id: "opencode2",
	home_environment_variable: "OPENCODE_CONFIG_DIR",
	login_args: [],
	LatestVersion: Effect.succeed(opencode2_certified_version),
	minimum_version: opencode2_certified_version,
	recommended_version: opencode2_certified_version,
	ResolveRelease: (version, target) =>
		Effect.gen(function* () {
			if (
				version !== opencode2_certified_version ||
				target.platform !== "win32" ||
				target.architecture !== "x64"
			)
				return yield* new ToolchainReleaseError({
					engine_id: "opencode2",
					reason:
						version === opencode2_certified_version
							? "platform_unsupported"
							: "version_unavailable",
				});
			return {
				archive_member: "package/bin/opencode2.exe",
				artifact_kind: "npm-tarball",
				binary: "opencode2.exe",
				integrity_sha512:
					"Z0oMvTBUhxmz1IYuQSMOZTpI2HoWjeIjdxJ39SoGrhDwvJZK7OI0rgIMYtDGavOucOQT8oxrazUiO4j+2hVMpw==",
				/** Bound the compressed response above the registry's 144,313,595-byte unpacked size. */
				size_bytes: 256 * 1024 * 1024,
				url: `https://registry.npmjs.org/@opencode-ai/cli-windows-x64/-/cli-windows-x64-${version}.tgz`,
				version,
			} satisfies ResolvedNpmTarballEngineRelease;
		}),
	vendor_home_directory: ".config/opencode",
};

export const hermes_certified_version = "0.20.5";
export const hermes_certified_commit = "2eaa863112d2980bbe6f15ea409a6a29e50964fe";

const HermesDistribution: EngineDistribution = {
	credential_files: [".env", "auth.json"],
	display_name: "Hermes",
	engine_id: "hermes",
	home_environment_variable: "HERMES_HOME",
	login_args: ["setup"],
	LatestVersion: Effect.succeed(hermes_certified_version),
	minimum_version: hermes_certified_version,
	recommended_version: hermes_certified_version,
	ResolveRelease: (version, target) =>
		Effect.gen(function* () {
			if (version !== hermes_certified_version)
				return yield* new ToolchainReleaseError({
					engine_id: "hermes",
					reason: "version_unavailable",
				});
			if (target.platform !== "win32" || target.architecture !== "x64")
				return yield* new ToolchainReleaseError({
					engine_id: "hermes",
					reason: "platform_unsupported",
				});
			return {
				artifact_kind: "staged-installer",
				binary: "hermes-agent/bin/hermes.exe",
				commit: hermes_certified_commit,
				installer_sha256:
					"e7521626d40f2d9fc2c51968244f22b3441dc4d5efebb28a0af4b335e91aecdf",
				size_bytes: 238_619,
				stages: [
					"uv",
					"python",
					"git",
					"node",
					"system-packages",
					"repository",
					"venv",
					"dependencies",
					"node-deps",
					"path",
					"config-templates",
					"platform-sdks",
					"bootstrap-marker",
				],
				url: `https://raw.githubusercontent.com/NousResearch/hermes-agent/${hermes_certified_commit}/scripts/install.ps1`,
				version,
			} satisfies ResolvedStagedInstallerEngineRelease;
		}),
	vendor_home_directory: "AppData/Local/hermes",
};

/** Every engine Artisan can own, in catalog order. */
export const engine_distributions: ReadonlyArray<EngineDistribution> = [
	CodexDistribution,
	ClaudeDistribution,
	OpenCode2Distribution,
	GrokDistribution,
	CursorDistribution,
	HermesDistribution,
];
