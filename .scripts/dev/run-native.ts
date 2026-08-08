import { spawnSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import { resolve } from "node:path";

const workspace_root = resolve(import.meta.dirname, "../..");
const development_root = resolve(workspace_root, ".dist", "dev");

const target = process.argv[2];
const forwarded = process.argv
	.slice(3)
	.filter((argument, index) => index !== 0 || argument !== "--");

const configuration =
	target === "cli"
		? {
				cargo_package: "artisan-editor-cli",
				environment: {
					ARTISAN_HOME: resolve(development_root, "forge-home"),
				},
			}
		: target === "installer"
			? {
					cargo_package: "ae-installer",
					environment: {
						ARTISAN_HOME: resolve(development_root, "install-root"),
						ARTISAN_INSTALL_ROOT: resolve(development_root, "install-root"),
					},
				}
			: undefined;

if (configuration === undefined) {
	throw new Error("Usage: node .scripts/dev/run-native.ts <cli|installer> [arguments...]");
}

for (const path of Object.values(configuration.environment)) mkdirSync(path, { recursive: true });

const result = spawnSync("cargo", ["run", "-p", configuration.cargo_package, "--", ...forwarded], {
	cwd: workspace_root,
	env: { ...process.env, ...configuration.environment },
	stdio: "inherit",
});

if (result.error !== undefined) throw result.error;
process.exitCode = result.status ?? 1;
