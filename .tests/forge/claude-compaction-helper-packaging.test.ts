import { spawn } from "node:child_process";
import { mkdtemp, mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";
import { build, type UserConfig } from "vite";

import forge_config from "../../forge.vite.config";

const temporary_directories: Array<string> = [];

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

const run_helper = (entry: string, input: string, environment: { readonly mailbox: string }) =>
	new Promise<string>((resolve, reject) => {
		const child = spawn(process.execPath, [entry], {
			cwd: process.cwd(),
			env: {
				...process.env,
				ARTISAN_CLAUDE_HOOK_CLAIM: "A".repeat(43),
				ARTISAN_CLAUDE_HOOK_MAILBOX: environment.mailbox,
				ARTISAN_CLAUDE_HOOK_RUN_ID: "packaged-helper-run",
			},
			stdio: ["pipe", "pipe", "pipe"],
		});
		const stdout: Array<Buffer> = [];
		const stderr: Array<Buffer> = [];
		child.stdout.on("data", (chunk: Buffer) => stdout.push(chunk));
		child.stderr.on("data", (chunk: Buffer) => stderr.push(chunk));
		child.once("error", reject);
		child.once("close", (code) =>
			code === 0
				? resolve(Buffer.concat(stdout).toString("utf8"))
				: reject(
						new Error(
							`Packaged helper exited ${String(code)}: ${Buffer.concat(stderr).toString("utf8")}`,
						),
					),
		);
		child.stdin.end(input);
	});

describe("Claude compaction helper packaging", () => {
	it("builds the configured helper entry beside the Forge host and executes it standalone", async () => {
		const configured = forge_config as UserConfig;
		const inputs = configured.build?.rollupOptions?.input as Record<string, string> | undefined;
		const helper_source = inputs?.["claude-post-compact-hook"];
		expect(helper_source).toMatch(/claude-post-compact-hook\.ts$/);
		expect(configured.build?.rollupOptions?.output).toMatchObject({
			entryFileNames: "[name].js",
			format: "es",
		});

		const root = await mkdtemp(join(tmpdir(), "artisan-packaged-claude-hook-"));
		temporary_directories.push(root);
		const output = join(root, "dist");
		await build({
			build: {
				emptyOutDir: true,
				outDir: output,
				rollupOptions: {
					input: { "claude-post-compact-hook": helper_source! },
					output: { entryFileNames: "[name].js", format: "es" },
				},
				ssr: true,
				target: "node22",
			},
			configFile: false,
			logLevel: "silent",
			ssr: { noExternal: true },
		});

		const helper = join(output, "claude-post-compact-hook.js");
		const mailbox = join(root, "mailbox");
		const transcript = join(root, "session.jsonl");
		await mkdir(mailbox);
		await writeFile(transcript, "{}\n");
		const stdout = await run_helper(
			helper,
			JSON.stringify({
				compact_summary: "packaged summary",
				cwd: process.cwd(),
				hook_event_name: "PostCompact",
				session_id: "session",
				transcript_path: transcript,
				trigger: "manual",
			}),
			{ mailbox },
		);
		expect(stdout).toBe("");

		const files = await readdir(mailbox);
		expect(files).toHaveLength(1);
		const record = JSON.parse(await readFile(join(mailbox, files[0]!), "utf8"));
		expect(record).toMatchObject({
			artisan_run_id: "packaged-helper-run",
			hook: {
				compact_summary: "packaged summary",
				hook_event_name: "PostCompact",
			},
		});
	});
});
