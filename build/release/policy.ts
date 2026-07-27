import { Schema } from "effect";

export const ReleaseMode = Schema.Literals(["dry-run", "release", "resume"]);
export type ReleaseMode = typeof ReleaseMode.Type;

export const ReleaseEvent = Schema.Literals(["pull_request", "push", "workflow_dispatch"]);
export type ReleaseEvent = typeof ReleaseEvent.Type;

export const FullCommit = Schema.String.check(Schema.isPattern(/^[a-f0-9]{40}$/));
export const SemanticVersion = Schema.String.check(
	Schema.isPattern(/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z.-]+)?$/),
);

export const candidate_ref = "refs/heads/candidate";

export type ReleaseLane = {
	readonly id: string;
	readonly runner: string;
	readonly bootstrap_asset: string;
	readonly cli_asset: string;
	readonly product_asset?: string;
	readonly state: "supported" | "planned";
	readonly reason?: string;
};

export const release_lanes: ReadonlyArray<ReleaseLane> = Object.freeze([
	{
		id: "windows-x64",
		runner: "windows-2025",
		bootstrap_asset: "artisan-bootstrap-windows-x64.exe",
		cli_asset: "ae-windows-x64.exe",
		product_asset: "artisan-{version}-windows-x64.zip",
		state: "supported",
	},
	{
		id: "windows-arm64",
		runner: "windows-2025",
		bootstrap_asset: "artisan-bootstrap-windows-arm64.exe",
		cli_asset: "ae-windows-arm64.exe",
		state: "planned",
		reason: "The Windows arm64 Editor/Forge payload is not qualified.",
	},
	{
		id: "macos-x64",
		runner: "macos-15-intel",
		bootstrap_asset: "artisan-bootstrap-macos-x64",
		cli_asset: "ae-macos-x64",
		state: "planned",
		reason: "The macOS product payload, signing, and notarization are not implemented.",
	},
	{
		id: "macos-arm64",
		runner: "macos-15",
		bootstrap_asset: "artisan-bootstrap-macos-arm64",
		cli_asset: "ae-macos-arm64",
		state: "planned",
		reason: "The macOS product payload, signing, and notarization are not implemented.",
	},
	{
		id: "linux-x64-gnu",
		runner: "ubuntu-24.04",
		bootstrap_asset: "artisan-bootstrap-linux-x64-gnu",
		cli_asset: "ae-linux-x64-gnu",
		state: "planned",
		reason: "The Linux glibc product payload is not implemented or qualified.",
	},
	{
		id: "linux-arm64-gnu",
		runner: "ubuntu-24.04-arm",
		bootstrap_asset: "artisan-bootstrap-linux-arm64-gnu",
		cli_asset: "ae-linux-arm64-gnu",
		state: "planned",
		reason: "The Linux arm64 glibc product payload is not implemented or qualified.",
	},
	{
		id: "linux-x64-musl",
		runner: "ubuntu-24.04",
		bootstrap_asset: "artisan-bootstrap-linux-x64-musl",
		cli_asset: "ae-linux-x64-musl",
		state: "planned",
		reason: "The Linux musl product payload is not implemented or qualified.",
	},
]);

export const transport_asset_names = (version: string): ReadonlyArray<string> => [
	`artisan-${version}-windows-x64.zip`,
	"artisan-bootstrap-windows-x64.exe",
	"artisan-bootstrap-windows-x64.exe.sha256",
	"release-manifest.json",
	"release-manifest.sig",
];
