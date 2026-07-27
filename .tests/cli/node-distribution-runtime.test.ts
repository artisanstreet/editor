import { spawn } from "node:child_process";
import { access, copyFile, mkdir, mkdtemp, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, expect, it } from "vitest";
import { Effect } from "effect";

import { DistributionOperations } from "../../modules/cli/src/distribution";
import {
	MakeWindowsDetachedUninstallPlan,
	make_node_distribution_runtime_layer,
} from "../../modules/cli/src/node-distribution-runtime";

describe("node distribution runtime", () => {
	it("builds a detached uninstall helper outside the locked product tree", () => {
		const plan = MakeWindowsDetachedUninstallPlan(
			"C:\\Users\\test\\AppData\\Local\\Artisan",
			4242,
			"C:\\Users\\test\\AppData\\Local\\Temp",
		);

		expect(plan.helper_path).toMatch(
			/^C:\\Users\\test\\AppData\\Local\\Temp\\artisan-uninstall-[a-f0-9-]+\.ps1$/u,
		);
		expect(plan.helper_path).not.toContain("\\Artisan\\");
		expect(plan.script).toContain("Wait-Process -Id 4242");
		expect(plan.script).toContain(
			"Remove-Item -LiteralPath 'C:\\Users\\test\\AppData\\Local\\Artisan' -Recurse -Force",
		);
		expect(plan.manual_cleanup_command).toContain(
			"Remove-Item -LiteralPath 'C:\\Users\\test\\AppData\\Local\\Artisan'",
		);
	});

	it.skipIf(process.platform !== "win32")(
		"waits for a locked product process before removing the version tree",
		async () => {
			const fixture_root = await mkdtemp(join(tmpdir(), "artisan-detached-uninstall-"));
			const product_root = join(fixture_root, "Artisan");
			const node_path = join(product_root, "versions", "0.1.0", "forge", "node.exe");
			await mkdir(join(product_root, "versions", "0.1.0", "forge"), {
				recursive: true,
			});
			await copyFile(process.execPath, node_path);
			const locked = spawn(node_path, ["-e", "setTimeout(() => {}, 400)"], {
				stdio: "ignore",
				windowsHide: true,
			});
			if (locked.pid === undefined) throw new Error("fixture process failed to start");
			const plan = MakeWindowsDetachedUninstallPlan(product_root, locked.pid, fixture_root);
			await writeFile(plan.helper_path, plan.script, "utf8");

			try {
				const helper = spawn(
					"powershell.exe",
					[
						"-NoLogo",
						"-NoProfile",
						"-NonInteractive",
						"-ExecutionPolicy",
						"Bypass",
						"-File",
						plan.helper_path,
					],
					{ stdio: "ignore", windowsHide: true },
				);
				await new Promise<void>((resolve, reject) => {
					helper.once("error", reject);
					helper.once("exit", (code) =>
						code === 0 ? resolve() : reject(new Error(`helper exited ${code}`)),
					);
				});
				await expect(access(product_root)).rejects.toMatchObject({ code: "ENOENT" });
				await expect(access(plan.failure_path)).rejects.toMatchObject({ code: "ENOENT" });
			} finally {
				if (!locked.killed) locked.kill();
				await rm(fixture_root, { force: true, recursive: true });
			}
		},
	);

	it("keeps doctor diagnostic and update fail-closed when release trust is absent", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-ae-distribution-"));
		try {
			const layer = make_node_distribution_runtime_layer({ ARTISAN_HOME: root }, "win32");
			const doctor = await Effect.runPromise(
				Effect.gen(function* () {
					return yield* (yield* DistributionOperations).Doctor();
				}).pipe(Effect.provide(layer)),
			);
			expect(doctor).toEqual({
				healthy: false,
				installation: "Absent",
				integrations: [],
			});
			await expect(
				Effect.runPromise(
					Effect.gen(function* () {
						return yield* (yield* DistributionOperations).Update();
					}).pipe(Effect.provide(layer)),
				),
			).rejects.toMatchObject({
				_tag: "DistributionOperationsError",
				code: "operation_unavailable",
				operation: "update",
			});
			expect(await readdir(root)).toEqual([]);
		} finally {
			await rm(root, { force: true, recursive: true });
		}
	});
});
