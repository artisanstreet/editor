import { InstallationManifest } from "@artisan/distribution";
import { Schema } from "effect";
import { describe, expect, it } from "vitest";

const valid_manifest = {
	format_version: 1,
	install_root: "C:\\Users\\sander\\AppData\\Local\\Artisan",
	platform: "windows",
	architecture: "x64",
	channel: "stable",
	activation_state: "active",
	finalization_state: "complete",
	active_version: "0.2.0",
	permanent_ae_path: "C:\\Users\\sander\\AppData\\Local\\Artisan\\bin\\ae.exe",
	previous_version: "0.1.0",
	artifact: {
		artifact_id: "windows-x64",
		sha256: "b".repeat(64),
		signing_key_id: "artisan-release-2026",
	},
	components: { editor: true, forge: true },
	integrations: {
		ae_path: {
			path: "C:\\Users\\sander\\AppData\\Local\\Artisan\\bin\\ae.exe",
			fingerprint: "ae:v1",
		},
	},
	transaction: { state: "idle" },
	installed_at: "2026-07-27T10:00:00.000Z",
	updated_at: "2026-07-27T10:05:00.000Z",
} as const;

describe("InstallationManifest", () => {
	it("tracks active and previous versions plus owned integrations", () => {
		expect(Schema.decodeUnknownSync(InstallationManifest)(valid_manifest)).toEqual(
			valid_manifest,
		);
	});

	it("accepts legacy format-v1 manifests without finalization state for store migration", () => {
		const { finalization_state: _, ...legacy } = valid_manifest;
		expect(Schema.decodeUnknownSync(InstallationManifest)(legacy)).toEqual(legacy);
	});

	it("requires resumable transaction metadata while activation is interrupted", () => {
		expect(() =>
			Schema.decodeUnknownSync(InstallationManifest)({
				...valid_manifest,
				transaction: { state: "staged", target_version: "0.3.0" },
			}),
		).toThrow();
	});

	it("supports a resumable first install before any artifact is active", () => {
		const {
			active_version,
			previous_version,
			artifact,
			permanent_ae_path: _,
			finalization_state: __,
			...common
		} = valid_manifest;
		const unactivated = {
			...common,
			activation_state: "unactivated",
			transaction: {
				state: "staged",
				target_version: "0.2.0",
				staging_path: "C:\\Users\\sander\\AppData\\Local\\Artisan\\staging\\0.2.0",
				started_at: "2026-07-27T10:01:00.000Z",
			},
		} as const;

		expect(Schema.decodeUnknownSync(InstallationManifest)(unactivated)).toEqual(unactivated);
		expect(active_version).toBe("0.2.0");
		expect(previous_version).toBe("0.1.0");
		expect(artifact.artifact_id).toBe("windows-x64");
	});

	it("rejects an idle manifest with no activated artifact", () => {
		const {
			active_version: _,
			previous_version: __,
			artifact: ___,
			...common
		} = valid_manifest;
		expect(() =>
			Schema.decodeUnknownSync(InstallationManifest)({
				...common,
				activation_state: "unactivated",
				transaction: { state: "idle" },
			}),
		).toThrow();
	});
});
