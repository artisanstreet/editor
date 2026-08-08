import { createHash } from "node:crypto";
import { readdirSync, realpathSync, statSync } from "node:fs";
import { dirname, extname, join, relative, resolve, sep } from "node:path";

import type { SeaBuildAssetInput } from "./sea/config";

const ToPortablePath = (path: string) => path.split(sep).join("/");

const AssetId = (relative_path: string) =>
	`artisan-${createHash("sha256").update(relative_path).digest("hex").slice(0, 24)}`;

const FilesBelow = (root: string) => {
	const files: Array<string> = [];
	const Visit = (directory: string) => {
		for (const entry of readdirSync(directory, { withFileTypes: true }).sort((left, right) =>
			left.name.localeCompare(right.name),
		)) {
			const path = resolve(directory, entry.name);
			if (entry.isDirectory()) Visit(path);
			else if (entry.isFile()) files.push(path);
		}
	};
	Visit(root);

	return files;
};

const MakeAsset = (source_path: string, relative_path: string): SeaBuildAssetInput => ({
	asset_id: AssetId(relative_path),
	executable: extname(source_path).toLowerCase() === ".exe",
	relative_path: ToPortablePath(relative_path),
	source_path,
});

const TreeAssets = (
	source_root: string,
	destination_root: string,
	include: (path: string) => boolean = () => true,
) =>
	FilesBelow(source_root)
		.filter(include)
		.map((source_path) =>
			MakeAsset(source_path, join(destination_root, relative(source_root, source_path))),
		);

const ExactAssets = (source_root: string, destination_root: string, paths: ReadonlyArray<string>) =>
	paths.map((path) => {
		const source_path = resolve(source_root, path);
		if (!statSync(source_path).isFile())
			throw new Error(`Forge SEA asset is not a file: ${source_path}`);

		return MakeAsset(source_path, join(destination_root, path));
	});

/** Collects only the Windows x64 runtime files Forge reads after SEA extraction. */
export const CollectForgeSeaAssets = (
	workspace_root: string,
): ReadonlyArray<SeaBuildAssetInput> => {
	const node_pty_root = resolve(workspace_root, "modules/backend/node_modules/node-pty");
	const koffi_root = realpathSync(resolve(workspace_root, "modules/engines/node_modules/koffi"));
	const koffi_native_root = resolve(dirname(koffi_root), "@koromix/koffi-win32-x64");
	const agent_sdk_root = realpathSync(
		resolve(workspace_root, "modules/engines/node_modules/@anthropic-ai/claude-agent-sdk"),
	);
	const claude_cli_path = resolve(
		dirname(agent_sdk_root),
		"claude-agent-sdk-win32-x64/claude.exe",
	);
	const assets = [
		...ExactAssets(node_pty_root, "native-runtime/node_modules/node-pty", [
			"LICENSE",
			"package.json",
		]),
		...TreeAssets(resolve(node_pty_root, "lib"), "native-runtime/node_modules/node-pty/lib"),
		...TreeAssets(
			resolve(node_pty_root, "prebuilds/win32-x64"),
			"native-runtime/node_modules/node-pty/prebuilds/win32-x64",
			(path) => extname(path).toLowerCase() !== ".pdb",
		),
		...ExactAssets(koffi_root, "native-runtime/node_modules/koffi", [
			"index.cjs",
			"package.json",
			"src/koffi/index.cjs",
			"src/koffi/src/static.cjs",
		]),
		...ExactAssets(koffi_native_root, "native-runtime/node_modules/@koromix/koffi-win32-x64", [
			"index.js",
			"package.json",
		]),
		...TreeAssets(
			resolve(koffi_native_root, "win32_x64"),
			"native-runtime/node_modules/@koromix/koffi-win32-x64/win32_x64",
		),
		MakeAsset(claude_cli_path, "native-runtime/node_modules/claude-agent-sdk/claude.exe"),
		...TreeAssets(resolve(workspace_root, "modules/backend/drizzle"), "migrations"),
	].sort((left, right) => left.relative_path.localeCompare(right.relative_path));

	const ids = new Set(assets.map((asset) => asset.asset_id));
	const paths = new Set(assets.map((asset) => asset.relative_path));
	if (ids.size !== assets.length || paths.size !== assets.length)
		throw new Error("Forge SEA assets must have unique ids and runtime paths");

	return assets;
};
