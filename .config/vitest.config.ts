import { configDefaults, defineConfig } from "vitest/config";

/**
 * Test files that launch real host processes or take exclusive host state:
 * engine process hosts (Claude/Codex CLIs through the Windows job machinery),
 * terminal shells, git executions, PowerShell ACL operations, and the
 * packaging/native suites. Under Windows' default pool these collide with
 * concurrent load and blow the fixed per-test timeout, which is what the old
 * blanket `maxWorkers: 1` protected against — at the cost of running the
 * entire suite, overwhelmingly in-process and parallel-safe, on one worker.
 */
const spawn_heavy_suites = [
	".tests/engines/**/*.test.ts",
	".tests/backend/terminal-*.test.ts",
	".tests/backend/git-*.test.ts",
	".tests/backend/private-file-permissions.test.ts",
	".tests/backend/host-identity.test.ts",
	".tests/backend/native-directory-picker.test.ts",
	".tests/cli/**/*.test.ts",
	".tests/build/**/*.test.ts",
	".tests/desktop/**/*.test.ts",
	".tests/distribution/**/*.test.ts",
	".tests/deep/**/*.test.ts",
	".tests/installer/**/*.test.ts",
	".tests/release/**/*.test.ts",
	".tests/install-transport/**/*.test.ts",
	".tests/platform/**/*.test.ts",
	".tests/dev/**/*.test.ts",
];

export default defineConfig({
	test: {
		environment: "node",
		testTimeout: 15_000,
		/**
		 * Windows splits the run into two phases instead of serializing all of
		 * it: everything parallel-safe fans out across the worker pool first,
		 * then the spawn-heavy files run alone, one at a time, with no
		 * concurrent load to contend with — the same determinism the blanket
		 * single worker bought, without taxing the other ~90% of the suite.
		 * Other platforms keep the single fully-parallel pool they always had.
		 */
		...(process.platform === "win32"
			? {
					projects: [
						{
							test: {
								environment: "node",
								exclude: [...configDefaults.exclude, ...spawn_heavy_suites],
								include: [".tests/**/*.test.ts"],
								name: "parallel",
								sequence: { groupOrder: 0 },
								testTimeout: 15_000,
							},
						},
						{
							test: {
								environment: "node",
								fileParallelism: false,
								include: spawn_heavy_suites,
								name: "spawn-serial",
								sequence: { groupOrder: 1 },
								testTimeout: 15_000,
							},
						},
					],
				}
			: { include: [".tests/**/*.test.ts"] }),
	},
});
