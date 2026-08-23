import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const workspace = resolve(import.meta.dirname, "../../..");

const Read = (path: string) => readFileSync(resolve(workspace, path), "utf8");

describe("Bazel scaffold", () => {
	it("pins Bazel, Node, and pnpm through Bzlmod", () => {
		expect(Read(".bazelversion").trim()).toBe("9.2.0");

		const module = Read("MODULE.bazel");
		expect(module).toContain('bazel_dep(name = "aspect_rules_js", version = "3.4.0")');
		expect(module).toContain('bazel_dep(name = "rules_nodejs", version = "6.7.5")');
		expect(module).toContain('node.toolchain(node_version = "24.18.0")');
		expect(module).toContain('pnpm.pnpm(pnpm_version_from = "//:package.json")');
		expect(module).toContain('use_repo(pnpm, "pnpm")');
	});

	it("translates the committed pnpm graph without host node_modules", () => {
		const module = Read("MODULE.bazel");
		const repository = Read("REPO.bazel");
		const defaults = Read(".config/bazel/defaults.bazelrc");
		const patch = Read("tools/bazel/bazel-lib-windows-native-patch.patch");

		expect(module).toContain('pnpm_lock = "//:pnpm-lock.yaml"');
		expect(module).toContain('"//:patches/@effect__platform-node-shared@4.0.0-beta.97.patch"');
		expect(module).toContain('"//:patches/pe-library@2.0.1.patch"');
		expect(module).toContain("run_lifecycle_hooks = False");
		expect(module).toContain('package = "@sveltejs/kit"');
		expect(module).toContain("presets = []");
		expect(module).toContain('module_name = "bazel_lib"');
		expect(module).toContain('use_repo(npm, "npm")');
		expect(repository).toContain('"**/node_modules"');
		expect(defaults).toContain("ASPECT_RULES_JS_FROZEN_PNPM_LOCK=1");
		expect(defaults).toContain("common --lockfile_mode=error");
		expect(defaults).toContain("build --action_env=SYSTEMROOT");
		expect(defaults).toContain("--experimental_windows_watchfs");
		expect(patch).toContain("repository_ctx.patch");
	});

	it("maps only existing non-server parity scripts to Bazel targets", () => {
		const build = Read("BUILD.bazel");
		const definitions = Read("tools/bazel/defs.bzl");

		for (const [target, script] of [
			["check", "check"],
			["check_frontend", "check:frontend"],
			["check_forge", "check:forge"],
			["forge_sea", "build:forge:sea"],
			["test", "test:bazel"],
			["check_native", "check:native"],
		]) {
			expect(build).toContain(`name = "${target}"`);
			expect(build).toContain(`script = "${script}"`);
		}

		expect(build).not.toContain('script = "dev"');
		expect(Read("package.json")).toContain(
			'"test:bazel": "pnpm run check:frontend && pnpm run check:forge && pnpm run test"',
		);
		expect(definitions).toContain('default = "@pnpm//:pkg"');
		expect(definitions).toContain('toolchains = ["@rules_nodejs//nodejs:toolchain_type"]');
		expect(definitions).toContain('ctx.actions.declare_directory("%s.workspace"');
		expect(definitions).toContain("publish = publish");
		expect(definitions).toContain('_workspace_targets(workspace_packages, "node_modules")');
		expect(definitions).toContain("_BAZEL_SUPPORT_FILES");
		expect(definitions).toContain("workspace_packages = workspace_packages");
		expect(definitions).toContain('execution_requirements = {"no-remote": "1"}');
		expect(definitions).toContain("use_default_shell_env = False");
		expect(definitions).toContain("use_host_path = False");
		expect(definitions).toContain(
			"use_default_shell_env = ctx.attr.use_default_shell_env or ctx.attr.use_host_path",
		);
		expect(definitions).toContain("ctx.configuration.default_shell_env");
		expect(definitions).toContain('"SYSTEMROOT"');
		const runner = Read("tools/bazel/run-pnpm-script.ts");
		expect(runner).toContain('"--config.verify-deps-before-run=false"');
		expect(runner).toContain("realpathSync.native(requested_workspace)");
		expect(runner).toContain('mkdtempSync(join(host_temporary_base, "artisan-bazel-"))');
		expect(runner).toContain("const ResolveBunExecutable");
		expect(runner).toContain('WriteBin("bunx", executable, ["x"], true)');
		expect(runner).toContain("rmSync(join(workspace, entry.name)");
		expect(build).toContain('".dist/forge/Artisan Broker.exe"');
		expect(build).toContain('".dist/forge/Artisan Forge.exe"');
		expect(build).toContain("use_host_path = True");
	});

	it("keeps the host-Cargo exception explicit", () => {
		const build = Read("BUILD.bazel");
		const defaults = Read(".config/bazel/defaults.bazelrc");

		expect(build).toContain("use_default_shell_env = True");
		expect(build).toContain('"requires-host-cargo"');
		expect(defaults).toContain("build --action_env=PATH");
		expect(defaults).toContain("build --action_env=RUSTUP_HOME");
	});

	it("provides every lockfile workspace importer as a rules_js package", () => {
		for (const module of [
			"backend",
			"catalog",
			"cli",
			"data",
			"desktop",
			"dev-tui",
			"distribution",
			"engines",
			"forge",
			"frontend",
			"installer",
			"protocol",
			"transport",
		]) {
			expect(Read(`modules/${module}/BUILD.bazel`)).toBe(
				'load("//tools/bazel:defs.bzl", "workspace_package")\n\nworkspace_package()\n',
			);
		}

		const definitions = Read("tools/bazel/defs.bzl");
		expect(definitions).toContain("def workspace_package():");
		expect(definitions).toContain('name = "pkg"');
		expect(definitions).toContain('npm_link_all_packages(name = "node_modules")');
		expect(definitions).toContain("js_library(");
	});
});
