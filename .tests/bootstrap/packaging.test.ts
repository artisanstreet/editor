import { execFile } from "node:child_process";
import { readFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { promisify } from "node:util";

import { describe, expect, it } from "vitest";
import { build } from "vite";

const ExecFile = promisify(execFile);
const bootstrap_root = resolve("modules/bootstrap");

describe("published bootstrap artifact", () => {
	it("builds a self-contained bundle without optional cloud or workspace runtime imports", async () => {
		/**
		 * `root` must be the bootstrap package, not the process working
		 * directory. The config declares a relative `outDir` of `.dist` with
		 * `emptyOutDir`, so building from the repository root would resolve it to
		 * the repository's own `.dist` and erase the desktop, frontend, and Forge
		 * build outputs that the deep suites run against.
		 */
		await build({
			configFile: join(bootstrap_root, "vite.config.ts"),
			logLevel: "silent",
			root: bootstrap_root,
		});

		const bundle = await readFile(join(bootstrap_root, ".dist", "entry.js"), "utf8");
		expect(bundle).not.toContain("@aws-sdk/client-s3");
		expect(bundle).not.toContain("unzipper");
		expect(bundle).not.toContain("@artisan/distribution");

		const package_json = JSON.parse(
			await readFile(join(bootstrap_root, "package.json"), "utf8"),
		) as {
			readonly dependencies?: Record<string, string>;
			readonly name?: string;
		};
		expect(package_json.name).toBe("artisan-editor");
		expect(package_json.dependencies).toBeUndefined();
	});

	it("packs the public artifact with only the executable bundle and metadata", async () => {
		const npm_cli = join(dirname(process.execPath), "node_modules", "npm", "bin", "npm-cli.js");
		const { stdout } = await ExecFile(
			process.execPath,
			[npm_cli, "pack", "--dry-run", "--json"],
			{ cwd: bootstrap_root },
		);
		const packed = JSON.parse(stdout) as ReadonlyArray<{
			readonly files: ReadonlyArray<{ readonly path: string }>;
			readonly name: string;
		}>;

		expect(packed).toHaveLength(1);
		expect(packed[0]?.name).toBe("artisan-editor");
		expect(packed[0]?.files.map(({ path }) => path).sort()).toEqual([
			".dist/entry.js",
			"package.json",
		]);
	});
});
