import { execFileSync, spawn } from "node:child_process";
import {
	cpSync,
	existsSync,
	mkdirSync,
	readFileSync,
	renameSync,
	rmSync,
	writeFileSync,
} from "node:fs";
import { resolve } from "node:path";

import { build } from "rolldown";
import { Schema } from "effect";

import { BrandForgeExecutable, CreateForgeSeaRolldownConfig } from "../../forge.rolldown.config.ts";
import { WindowsProcessHostModeArgument } from "../../modules/engines/src/process/windows-process-host-mode.ts";
import {
	ForgeSeaRuntimeSmokeModeArgument,
	ForgeSeaSmokeModeArgument,
} from "../../modules/forge/src/executable-runtime.ts";
import { CollectForgeSeaAssets } from "./forge-sea-assets.ts";
import { GenerateSeaBuildArtifacts } from "./sea/config.ts";

const workspace_root = resolve(import.meta.dirname, "../..");
const build_root = resolve(workspace_root, ".dist/forge-sea-build");
const output_root = resolve(workspace_root, ".dist/forge");
const staging_root = resolve(workspace_root, ".dist", `forge-sea-output-${process.pid}`);
const backup_root = resolve(workspace_root, ".dist", `forge-sea-backup-${process.pid}`);
const runtime_smoke_root = resolve(build_root, `runtime-smoke-${process.pid}`);
const bundle_path = resolve(build_root, "forge-main.cjs");
const blob_path = resolve(build_root, "forge.blob");
const manifest_path = resolve(build_root, "asset-manifest.json");
const config_path = resolve(build_root, "sea-config.json");
const executable_path = resolve(staging_root, "Artisan Forge.exe");
const manifest_asset_id = "artisan-sea-manifest";
const sea_sentinel = "NODE_SEA_FUSE_fce680ab2cc467b6e072b8b5df1996b2";

const SeaSmokeReport = Schema.Struct({
	assets: Schema.Int.check(Schema.isGreaterThan(0)),
	manifest_version: Schema.Literal(1),
	sea: Schema.Literal(true),
});

const SeaRuntimeSmokeReport = Schema.Struct({
	claude_cli: Schema.Literal(true),
	koffi: Schema.Literal(true),
	migrations: Schema.Int.check(Schema.isGreaterThan(0)),
	node_pty: Schema.Literal(true),
	runtime: Schema.Literal(true),
	sea: Schema.Literal(true),
});

const ProcessHostResponse = Schema.Union([
	Schema.Struct({ claim_token: Schema.NonEmptyString, type: Schema.Literal("claim_ack") }),
	Schema.Struct({ process_id: Schema.Int, type: Schema.Literal("ready") }),
	Schema.Struct({ message: Schema.String, type: Schema.Literal("spawn_error") }),
]);

const CanonicalJson = (value: unknown) => `${JSON.stringify(value, undefined, "\t")}\n`;
const RemoveTree = (path: string) =>
	rmSync(path, { force: true, maxRetries: 20, recursive: true, retryDelay: 250 });

const SmokeWindowsProcessHost = (executable: string, environment: NodeJS.ProcessEnv) =>
	new Promise<void>((accept, reject) => {
		const child = spawn(executable, [WindowsProcessHostModeArgument], {
			cwd: workspace_root,
			env: environment,
			stdio: ["pipe", "pipe", "pipe", "ipc"],
			windowsHide: true,
		});
		let acknowledged = false;
		let ready = false;
		let settled = false;
		let stdout = "";
		let stderr = "";
		const Finish = (cause?: unknown) => {
			if (settled) return;
			settled = true;
			clearTimeout(timeout);
			if (cause === undefined) accept();
			else {
				if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
				reject(cause);
			}
		};
		const Send = (message: unknown) =>
			child.send(message, (cause) => {
				if (cause) Finish(cause);
			});
		const timeout = setTimeout(
			() => Finish(new Error(`Forge SEA process-host smoke timed out: ${stderr}`)),
			60_000,
		);
		child.stdout?.setEncoding("utf8");
		child.stderr?.setEncoding("utf8");
		child.stdout?.on("data", (chunk: string) => {
			stdout = `${stdout}${chunk}`.slice(-4_096);
		});
		child.stderr?.on("data", (chunk: string) => {
			stderr = `${stderr}${chunk}`.slice(-4_096);
		});
		child.once("error", Finish);
		child.once("spawn", () => Send({ claim_token: "sea-build-smoke", type: "claim" }));
		child.on("message", (unknown_message) => {
			try {
				const message = Schema.decodeUnknownSync(ProcessHostResponse)(unknown_message);
				if (message.type === "spawn_error") {
					Finish(new Error(message.message));
				} else if (message.type === "claim_ack") {
					acknowledged = message.claim_token === "sea-build-smoke";
					Send({
						input: {
							args: ["-e", "process.stdout.write('artisan-sea-host-ok')"],
							command: process.execPath,
							cwd: workspace_root,
						},
						type: "start",
					});
				} else ready = message.process_id > 0;
			} catch (cause) {
				Finish(cause);
			}
		});
		child.once("exit", (code) => {
			if (code === 0 && acknowledged && ready && stdout === "artisan-sea-host-ok") Finish();
			else
				Finish(
					new Error(
						`Forge SEA process-host smoke failed (${String(code)}): ${stderr || stdout}`,
					),
				);
		});
	});

const PublishForgeSea = () => {
	RemoveTree(backup_root);
	const had_previous = existsSync(output_root);
	if (had_previous) renameSync(output_root, backup_root);
	try {
		renameSync(staging_root, output_root);
	} catch (cause) {
		if (had_previous && !existsSync(output_root)) {
			try {
				renameSync(backup_root, output_root);
			} catch (rollback_cause) {
				throw new AggregateError(
					[cause, rollback_cause],
					`Forge SEA publish failed; the previous artifact remains at ${backup_root}`,
				);
			}
		}
		throw cause;
	}
	if (had_previous) {
		try {
			RemoveTree(backup_root);
		} catch (cause) {
			console.warn(`[forge-build] left the replaced SEA at ${backup_root} (${cause})`);
		}
	}
};

export const BuildForgeSea = async () => {
	if (process.platform !== "win32" || process.arch !== "x64")
		throw new Error("Artisan Forge SEA production builds currently require Windows x64");

	await build(CreateForgeSeaRolldownConfig());
	const artifacts = GenerateSeaBuildArtifacts({
		assets: CollectForgeSeaAssets(workspace_root),
		main_path: bundle_path,
		output_path: blob_path,
	});
	writeFileSync(manifest_path, artifacts.asset_manifest_json);
	writeFileSync(
		config_path,
		CanonicalJson({
			...artifacts.sea_config,
			assets: {
				...artifacts.sea_config.assets,
				[manifest_asset_id]: manifest_path,
			},
		}),
	);

	execFileSync(process.execPath, ["--experimental-sea-config", config_path], {
		cwd: workspace_root,
		stdio: "inherit",
	});
	RemoveTree(staging_root);
	mkdirSync(staging_root, { recursive: true });
	try {
		cpSync(process.execPath, executable_path);
		const workspace = Schema.decodeUnknownSync(
			Schema.Struct({ version: Schema.NonEmptyString }),
		)(JSON.parse(readFileSync(resolve(workspace_root, "package.json"), "utf8")));
		BrandForgeExecutable(executable_path, workspace.version);
		execFileSync(
			process.execPath,
			[
				resolve(workspace_root, "node_modules/postject/dist/cli.js"),
				executable_path,
				"NODE_SEA_BLOB",
				blob_path,
				"--sentinel-fuse",
				sea_sentinel,
				"--overwrite",
			],
			{ cwd: workspace_root, stdio: "inherit" },
		);
		const smoke = execFileSync(executable_path, [ForgeSeaSmokeModeArgument], {
			encoding: "utf8",
			timeout: 60_000,
			windowsHide: true,
		});
		Schema.decodeUnknownSync(SeaSmokeReport)(JSON.parse(smoke));
		RemoveTree(runtime_smoke_root);
		const smoke_environment = {
			...process.env,
			ARTISAN_SEA_CACHE_ROOT: runtime_smoke_root,
		};
		try {
			const runtime_smoke = execFileSync(
				executable_path,
				[ForgeSeaRuntimeSmokeModeArgument],
				{
					encoding: "utf8",
					env: smoke_environment,
					timeout: 120_000,
					windowsHide: true,
				},
			);
			Schema.decodeUnknownSync(SeaRuntimeSmokeReport)(JSON.parse(runtime_smoke));
			await SmokeWindowsProcessHost(executable_path, smoke_environment);
		} finally {
			RemoveTree(runtime_smoke_root);
		}

		PublishForgeSea();
	} finally {
		RemoveTree(staging_root);
	}
};
