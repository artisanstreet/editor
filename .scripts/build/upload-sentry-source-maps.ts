import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, readdirSync } from "node:fs";
import { extname, relative, resolve } from "node:path";

const repository_root = resolve(import.meta.dirname, "..", "..");
const frontend_root = resolve(repository_root, ".dist", "frontend");
const staged_frontend_root = resolve(repository_root, ".dist", "desktop", "frontend");
const desktop_main = resolve(repository_root, ".dist", "desktop", "main.js");
const desktop_main_map = `${desktop_main}.map`;
const forge_root = resolve(repository_root, ".dist", "forge-sea-build");

/**
 * Uploads are opt-in and never a build failure. A machine without the full
 * credential set — including one with only a token or org in its shell —
 * simply skips; a build must succeed identically with or without Sentry.
 */
const required_environment = [
  "SENTRY_AUTH_TOKEN",
  "SENTRY_ORG",
  "SENTRY_EDITOR_PROJECT",
  "SENTRY_FORGE_PROJECT",
  "ARTISAN_RELEASE_VERSION",
  "ARTISAN_RELEASE_COMMIT",
] as const;

const is_configured = (name: string) => (process.env[name]?.length ?? 0) > 0;
const missing = required_environment.filter((name) => !is_configured(name));
if (missing.length > 0) {
  console.log(`[sentry] source-map upload skipped (not configured: ${missing.join(", ")})`);
  process.exit(0);
}

const environment = process.env as NodeJS.ProcessEnv &
  Record<(typeof required_environment)[number], string>;
const executable = process.platform === "win32" ? "sentry-cli.cmd" : "sentry-cli";
const RunSentry = (arguments_: ReadonlyArray<string>) => {
  const result = spawnSync(executable, arguments_, {
    cwd: repository_root,
    env: process.env,
    /** Node refuses to spawn .cmd shims without a shell (CVE-2024-27980 hardening). */
    shell: process.platform === "win32",
    stdio: "inherit",
  });
  if (result.error !== undefined) throw result.error;
  if (result.status !== 0) {
    throw new Error(`sentry-cli exited with status ${result.status ?? "unknown"}`);
  }
};

const CopyInjectedFrontendJavaScript = (directory: string) => {
  for (const entry of readdirSync(directory, { recursive: true, withFileTypes: true })) {
    if (!entry.isFile()) continue;
    const extension = extname(entry.name).toLowerCase();
    if (extension !== ".js" && extension !== ".mjs") continue;
    const source = resolve(entry.parentPath, entry.name);
    const destination = resolve(staged_frontend_root, relative(frontend_root, source));
    if (!existsSync(destination)) {
      throw new Error(`Staged renderer file is missing before Sentry injection: ${destination}`);
    }
    copyFileSync(source, destination);
  }
};

const version = environment.ARTISAN_RELEASE_VERSION;
const commit = environment.ARTISAN_RELEASE_COMMIT;
const editor_release = `artisan-editor@${version}+${commit}`;
const forge_release = `artisan-forge@${version}+${commit}`;

RunSentry(["sourcemaps", "inject", frontend_root]);
CopyInjectedFrontendJavaScript(frontend_root);
RunSentry(["sourcemaps", "inject", desktop_main, desktop_main_map]);

RunSentry([
  "sourcemaps",
  "upload",
  "--org",
  environment.SENTRY_ORG,
  "--project",
  environment.SENTRY_EDITOR_PROJECT,
  "--release",
  editor_release,
  "--dist",
  commit,
  "--strip-common-prefix",
  frontend_root,
  desktop_main,
  desktop_main_map,
]);
RunSentry([
  "sourcemaps",
  "upload",
  "--org",
  environment.SENTRY_ORG,
  "--project",
  environment.SENTRY_FORGE_PROJECT,
  "--release",
  forge_release,
  "--dist",
  commit,
  "--strip-common-prefix",
  forge_root,
]);

console.log(`[sentry] uploaded private source maps for ${editor_release} and ${forge_release}`);
