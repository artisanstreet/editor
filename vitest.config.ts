import { defineConfig } from "vitest/config";

export default defineConfig({
	test: {
		environment: "node",
		include: [".tests/**/*.test.ts"],
		testTimeout: 15_000,
		/**
		 * Concurrent PowerShell ACL operations and Codex process hosts exceed the
		 * fixed per-test timeout when the full suite uses Windows' default pool.
		 * Run Windows test files through one worker so the suite remains
		 * deterministic without changing other platforms.
		 */
		...(process.platform === "win32" ? { maxWorkers: 1 } : {}),
	},
});
