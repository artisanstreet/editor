import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

const root = new URL("../..", import.meta.url);
const source = (path: string) => readFileSync(new URL(path, root), "utf8");

describe("observability release artifacts", () => {
  it("generates private source maps for every JavaScript runtime", () => {
    expect(source("modules/frontend/vite.config.ts")).toContain('sourcemap: "hidden"');
    expect(source(".config/desktop.vite.config.ts")).toContain('sourcemap: "hidden"');
    expect(source(".config/forge.rolldown.config.ts")).toContain('sourcemap: "hidden"');
  });

  it("keeps source maps out of the installed Electron and Forge payloads", () => {
    const desktop = source(".config/desktop.vite.config.ts");
    const forge_assets = source(".scripts/build/forge-sea-assets.ts");

    expect(desktop).toContain('!source.endsWith(".map")');
    expect(forge_assets).not.toMatch(/relative_path:\s*[^\n]*\.map/u);
  });

  it("propagates one release version and commit to Editor and Forge", () => {
    const runner = source(".scripts/build/runner.ts");
    const forge = source(".config/forge.rolldown.config.ts");

    expect(runner).toContain("ARTISAN_RELEASE_COMMIT");
    expect(runner).toContain("ARTISAN_RELEASE_VERSION");
    expect(forge).toContain("process.env.ARTISAN_RELEASE_VERSION");
    expect(forge).toContain("process.env.ARTISAN_RELEASE_COMMIT");
  });

  it("finalizes the exact Sentry CSP in plain and precompressed frontend artifacts", () => {
    const config = source("modules/frontend/vite.config.ts");
    expect(config).toContain("closeBundle");
    expect(config).toContain("__ARTISAN_SENTRY_CONNECT_ORIGIN__");
    expect(config).toContain("brotliCompressSync");
    expect(config).toContain("gzipSync");
  });

  it("uploads maps only from an explicitly credentialed release environment", () => {
    const uploader = source(".scripts/build/upload-sentry-source-maps.ts");

    expect(uploader).toContain("SENTRY_AUTH_TOKEN");
    expect(uploader).toContain("SENTRY_ORG");
    expect(uploader).toContain("SENTRY_EDITOR_PROJECT");
    expect(uploader).toContain("SENTRY_FORGE_PROJECT");
    expect(uploader).toContain("ARTISAN_RELEASE_VERSION");
    expect(uploader).toContain("ARTISAN_RELEASE_COMMIT");
    expect(uploader).not.toContain("[REDACTED]");
  });

  it("injects the Forge debug ID before creating the SEA blob", () => {
    const forge_sea = source(".scripts/build/build-forge-sea.ts");
    const uploader = source(".scripts/build/upload-sentry-source-maps.ts");
    const injection = forge_sea.indexOf("InjectForgeDebugIds(build_root)");
    const blob = forge_sea.indexOf("GenerateSeaBuildArtifacts({");

    expect(injection).toBeGreaterThan(-1);
    expect(blob).toBeGreaterThan(injection);
    expect(uploader).not.toContain('RunSentry(["sourcemaps", "inject", forge_root])');
  });
});
