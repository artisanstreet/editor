import { spawnSync } from "node:child_process";
import {
	copyFileSync,
	chmodSync,
	existsSync,
	lstatSync,
	mkdtempSync,
	mkdirSync,
	readFileSync,
	readdirSync,
	realpathSync,
	renameSync,
	rmSync,
	statSync,
	symlinkSync,
	unlinkSync,
	writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import {
	basename,
	delimiter,
	dirname,
	extname,
	isAbsolute,
	join,
	relative as relativePath,
	resolve,
	sep,
} from "node:path";

interface WorkspaceInput {
	readonly relative: string;
	readonly source: string;
}

const IsRecord = (value: unknown): value is Record<string, unknown> =>
	typeof value === "object" && value !== null && !Array.isArray(value);

const ParseJson = (value: string): unknown => JSON.parse(value) as unknown;

const DecodeInputs = (value: unknown): ReadonlyArray<WorkspaceInput> => {
	if (!Array.isArray(value)) throw new Error("Bazel workspace input manifest must be an array");
	return value.map((input) => {
		if (
			!IsRecord(input) ||
			typeof input.source !== "string" ||
			typeof input.relative !== "string"
		)
			throw new Error("Bazel workspace input manifest contains an invalid entry");
		return { relative: input.relative, source: input.source };
	});
};

const DecodePublishPaths = (value: unknown): ReadonlyArray<string> => {
	if (!Array.isArray(value) || value.some((path) => typeof path !== "string"))
		throw new Error("Bazel publish paths must be an array of strings");
	return value as ReadonlyArray<string>;
};

const DecodeWorkspacePackages = (value: unknown): ReadonlyArray<string> => {
	if (
		!Array.isArray(value) ||
		value.some((path) => typeof path !== "string" || !/^[a-z0-9][a-z0-9-]*$/u.test(path))
	)
		throw new Error("Bazel workspace packages must be safe directory names");
	return value as ReadonlyArray<string>;
};

const [
	pnpm_path,
	script,
	workspace_path,
	inputs_path,
	publish_json,
	workspace_packages_json,
	use_host_environment_json,
	use_host_path_json,
] = process.argv.slice(2);
if (
	pnpm_path === undefined ||
	script === undefined ||
	workspace_path === undefined ||
	inputs_path === undefined ||
	publish_json === undefined ||
	workspace_packages_json === undefined ||
	use_host_environment_json === undefined ||
	use_host_path_json === undefined
)
	throw new Error(
		"Expected pnpm, script, workspace, input manifest, publish paths, packages, environment mode, and host-path mode",
	);

const requested_workspace = resolve(workspace_path);
mkdirSync(requested_workspace, { recursive: true });
// Windows module URLs are case-sensitive strings even though the filesystem is
// not. Use the native spelling before wiring package junctions so Vite cannot
// load one workspace package once through `C:\\users` and again through
// `C:\\Users` as two class/module identities.
const workspace = realpathSync.native(requested_workspace);
const pnpm = resolve(pnpm_path);
const IsBelowWorkspace = (path: string) => {
	const relative = relativePath(workspace, path);
	return (
		relative !== "" &&
		relative !== ".." &&
		!relative.startsWith(`..${sep}`) &&
		!isAbsolute(relative)
	);
};

const inputs = DecodeInputs(ParseJson(readFileSync(inputs_path, "utf8")));
const publish = DecodePublishPaths(ParseJson(publish_json));
const workspace_packages = DecodeWorkspacePackages(ParseJson(workspace_packages_json));
const use_host_environment = ParseJson(use_host_environment_json);
if (typeof use_host_environment !== "boolean")
	throw new Error("Bazel host environment mode must be a boolean");
const use_host_path = ParseJson(use_host_path_json);
if (typeof use_host_path !== "boolean") throw new Error("Bazel host-path mode must be a boolean");

const StageLinkedEntry = (source: string, destination: string) => {
	const metadata = statSync(source);
	if (metadata.isDirectory()) {
		symlinkSync(
			resolve(source),
			destination,
			process.platform === "win32" ? "junction" : "dir",
		);
		return;
	}
	copyFileSync(source, destination);
	if (process.platform !== "win32") {
		const mode = metadata.mode & 0o777;
		if (mode !== 0) chmodSync(destination, mode);
	}
};

/** Keeps dependency packages linked while making scopes and `.bin` action-local. */
const StageNodeModules = (source: string, destination: string) => {
	mkdirSync(destination, { recursive: true });
	for (const entry of readdirSync(source, { withFileTypes: true })) {
		if (entry.name === ".bin") continue;
		const source_entry = join(source, entry.name);
		const destination_entry = join(destination, entry.name);
		if (entry.name.startsWith("@") && statSync(source_entry).isDirectory()) {
			mkdirSync(destination_entry, { recursive: true });
			for (const scoped of readdirSync(source_entry, { withFileTypes: true })) {
				StageLinkedEntry(
					join(source_entry, scoped.name),
					join(destination_entry, scoped.name),
				);
			}
		} else StageLinkedEntry(source_entry, destination_entry);
	}
};

for (const input of inputs) {
	const destination = resolve(workspace, input.relative);
	if (!IsBelowWorkspace(destination))
		throw new Error(`Bazel workspace input path is unsafe: ${input.relative}`);
	mkdirSync(dirname(destination), { recursive: true });
	if (
		input.relative === "node_modules" ||
		input.relative.replaceAll("\\", "/").endsWith("/node_modules")
	)
		StageNodeModules(input.source, destination);
	else StageLinkedEntry(input.source, destination);
}

const PackageNameParts = (name: string) => {
	const parts = name.split("/");
	const valid = name.startsWith("@")
		? parts.length === 2 && parts.every((part) => /^@?[a-z0-9][a-z0-9._-]*$/u.test(part))
		: parts.length === 1 && /^[a-z0-9][a-z0-9._-]*$/u.test(name);
	if (!valid) throw new Error(`Workspace package name is unsafe: ${name}`);
	return parts;
};

const node_modules_roots = [
	join(workspace, "node_modules"),
	...workspace_packages.map((directory) => join(workspace, "modules", directory, "node_modules")),
].filter(existsSync);
for (const directory of workspace_packages) {
	const package_root = join(workspace, "modules", directory);
	const manifest = ParseJson(readFileSync(join(package_root, "package.json"), "utf8"));
	if (!IsRecord(manifest) || typeof manifest.name !== "string")
		throw new Error(`Workspace package ${directory} has no valid package name`);
	const package_name_parts = PackageNameParts(manifest.name);
	for (const node_modules_root of node_modules_roots) {
		const destination = join(node_modules_root, ...package_name_parts);
		if (!existsSync(destination)) continue;
		if (!lstatSync(destination).isSymbolicLink())
			throw new Error(`Workspace package link is not replaceable: ${destination}`);
		unlinkSync(destination);
		symlinkSync(package_root, destination, process.platform === "win32" ? "junction" : "dir");
	}
}

const bin_root = join(workspace, "node_modules", ".bin");
mkdirSync(bin_root, { recursive: true });
const WriteBin = (
	name: string,
	target: string,
	fixed_arguments: ReadonlyArray<string> = [],
	native_executable = false,
) => {
	if (name.includes("/") || name.includes("\\")) return;
	const quoted_arguments = fixed_arguments.map((argument) => ` "${argument}"`).join("");
	if (process.platform === "win32") {
		const command =
			extname(target).toLowerCase() === ".exe"
				? `"${target}"${quoted_arguments}`
				: `"${process.execPath}" "${target}"${quoted_arguments}`;
		writeFileSync(join(bin_root, `${name}.cmd`), `@echo off\r\n${command} %*\r\n`);
		return;
	}
	const path = join(bin_root, name);
	const executable = native_executable ? target : process.execPath;
	const script =
		executable === target
			? `#!/usr/bin/env sh\nexec "${target}"${quoted_arguments} "$@"\n`
			: `#!/usr/bin/env sh\nexec "${executable}" "${target}"${quoted_arguments} "$@"\n`;
	writeFileSync(path, script);
	chmodSync(path, 0o755);
};

const ResolveBunExecutable = (package_root: string) => {
	const oven_scope = join(dirname(realpathSync.native(package_root)), "@oven");
	if (!existsSync(oven_scope))
		throw new Error("Bazel's translated Bun package has no installed platform runtime");
	const candidates = readdirSync(oven_scope, { withFileTypes: true })
		.filter((entry) => entry.isDirectory() || entry.isSymbolicLink())
		.map((entry) => join(oven_scope, entry.name))
		.sort((left, right) => {
			const left_baseline = basename(left).includes("-baseline");
			const right_baseline = basename(right).includes("-baseline");
			return Number(left_baseline) - Number(right_baseline) || left.localeCompare(right);
		});
	for (const candidate of candidates) {
		const manifest = ParseJson(readFileSync(join(candidate, "package.json"), "utf8"));
		if (!IsRecord(manifest) || !Array.isArray(manifest.os) || !Array.isArray(manifest.cpu))
			continue;
		if (!manifest.os.includes(process.platform) || !manifest.cpu.includes(process.arch))
			continue;
		const executable = join(candidate, "bin", process.platform === "win32" ? "bun.exe" : "bun");
		if (!existsSync(executable)) continue;
		const probe = spawnSync(executable, ["--version"], { stdio: "ignore" });
		if (probe.status === 0) return executable;
	}
	throw new Error("Bazel's translated Bun package has no compatible platform runtime");
};

const ReadPackageBins = (package_root: string) => {
	const package_json = join(package_root, "package.json");
	if (!existsSync(package_json)) return;
	const manifest = ParseJson(readFileSync(package_json, "utf8"));
	if (!IsRecord(manifest)) return;
	if (manifest.name === "bun") {
		const executable = ResolveBunExecutable(package_root);
		WriteBin("bun", executable, [], true);
		WriteBin("bunx", executable, ["x"], true);
		return;
	}
	if (typeof manifest.bin === "string") {
		WriteBin(basename(package_root), resolve(package_root, manifest.bin));
	} else if (IsRecord(manifest.bin)) {
		for (const [name, path] of Object.entries(manifest.bin)) {
			if (typeof path === "string") WriteBin(name, resolve(package_root, path));
		}
	}
};

for (const entry of readdirSync(join(workspace, "node_modules"), { withFileTypes: true })) {
	if (entry.name === ".bin" || entry.name === ".aspect_rules_js") continue;
	const package_root = join(workspace, "node_modules", entry.name);
	if (entry.name.startsWith("@")) {
		for (const scoped of readdirSync(package_root, { withFileTypes: true })) {
			ReadPackageBins(join(package_root, scoped.name));
		}
	} else ReadPackageBins(package_root);
}
WriteBin("pnpm", pnpm);

const windows_user_temp =
	process.platform === "win32" && process.env.USERPROFILE !== undefined
		? join(process.env.USERPROFILE, "AppData", "Local", "Temp")
		: undefined;
const host_temporary_base =
	windows_user_temp !== undefined && existsSync(windows_user_temp) ? windows_user_temp : tmpdir();
/**
 * Git subprocesses intentionally scrub every inherited `GIT_*` variable, so a
 * test temp nested below Bazel's `_main/.git` could be misidentified as the
 * source repository. Host-tool integration tests use a unique, lifecycle-owned
 * temp outside the execroot; strict build actions keep theirs in the output.
 */
const temporary_root = use_host_path
	? mkdtempSync(join(host_temporary_base, "artisan-bazel-"))
	: join(workspace, ".tmp");
const hermetic_home = join(workspace, ".home");
mkdirSync(temporary_root, { recursive: true });
mkdirSync(join(hermetic_home, "AppData", "Local"), { recursive: true });
mkdirSync(join(hermetic_home, "AppData", "Roaming"), { recursive: true });
const system_root = process.env.SYSTEMROOT;
const platform_paths =
	process.platform === "win32" && system_root !== undefined
		? [
				join(system_root, "System32", "WindowsPowerShell", "v1.0"),
				join(system_root, "System32"),
			]
		: [];
const environment: NodeJS.ProcessEnv = {
	...process.env,
	...(use_host_environment
		? {}
		: {
				APPDATA: join(hermetic_home, "AppData", "Roaming"),
				GIT_CEILING_DIRECTORIES: dirname(workspace),
				HOME: hermetic_home,
				LOCALAPPDATA: join(hermetic_home, "AppData", "Local"),
				USERPROFILE: hermetic_home,
			}),
	...(process.platform === "win32" && system_root !== undefined
		? {
				COMSPEC: join(system_root, "System32", "cmd.exe"),
				PATHEXT: ".COM;.EXE;.BAT;.CMD",
			}
		: {}),
	PATH: [dirname(process.execPath), ...platform_paths, process.env.PATH]
		.filter(Boolean)
		.join(delimiter),
	TEMP: temporary_root,
	TMP: temporary_root,
};
const result = (() => {
	try {
		return spawnSync(
			process.execPath,
			[pnpm, "--config.verify-deps-before-run=false", "run", script],
			{
				cwd: workspace,
				env: environment,
				stdio: "inherit",
			},
		);
	} finally {
		if (use_host_path) rmSync(temporary_root, { force: true, recursive: true });
	}
})();
if (result.error !== undefined) throw result.error;
if (result.status !== 0) process.exit(result.status ?? 1);

const safe_publish_component = /^[A-Za-z0-9@._ -]+$/u;
const IsSafePublishPath = (path: string) =>
	path.length > 0 &&
	!path.includes("\\") &&
	!isAbsolute(path) &&
	path
		.split("/")
		.every(
			(component) =>
				component !== "" &&
				component !== "." &&
				component !== ".." &&
				safe_publish_component.test(component) &&
				!/[ .]$/u.test(component),
		);

const publish_root = join(workspace, ".artisan-bazel-publish");
mkdirSync(publish_root, { recursive: true });
for (const [index, relative] of publish.entries()) {
	if (!IsSafePublishPath(relative)) throw new Error(`Bazel publish path is unsafe: ${relative}`);
	const source = resolve(workspace, ...relative.split("/"));
	if (!IsBelowWorkspace(source)) throw new Error(`Bazel publish path escaped: ${relative}`);
	if (!existsSync(source)) throw new Error(`Bazel publish artifact is missing: ${relative}`);
	renameSync(source, join(publish_root, String(index)));
}
for (const entry of readdirSync(workspace, { withFileTypes: true })) {
	if (entry.name === ".artisan-bazel-publish") continue;
	rmSync(join(workspace, entry.name), { force: true, recursive: true });
}
for (const [index, relative] of publish.entries()) {
	const destination = resolve(workspace, ...relative.split("/"));
	mkdirSync(dirname(destination), { recursive: true });
	renameSync(join(publish_root, String(index)), destination);
}
rmSync(publish_root, { force: true, recursive: true });

writeFileSync(
	join(workspace, ".artisan-bazel.stamp"),
	`${JSON.stringify({ script, version: 1 })}\n`,
);
