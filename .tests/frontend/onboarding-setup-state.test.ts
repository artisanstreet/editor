import { describe, expect, it } from "vitest";

import type { EngineInstallationReport, EngineUsageReport } from "@artisan/protocol";
import {
	ProjectManagedHarnessSetup,
} from "../../modules/frontend/src/routes/components/onboarding/setup-state";

const installation = (
	overrides: Partial<EngineInstallationReport> = {},
): EngineInstallationReport => ({
	activity: "idle",
	credentials_present: false,
	display_name: "Codex",
	engine_id: "codex",
	managed: false,
	...overrides,
});

const usage = (
	overrides: Partial<EngineUsageReport> = {},
): EngineUsageReport => ({
	authentication: "unauthenticated",
	display_name: "Codex",
	engine_id: "codex",
	windows: [],
	...overrides,
});

describe("onboarding setup state", () => {
	it("offers installation for an unmanaged harness", () => {
		expect(
			ProjectManagedHarnessSetup({
				available: true,
				pending: false,
				report: installation(),
			}),
		).toMatchObject({ action: "install", label: "Download", ready: false });
	});

	it("projects live installation phases", () => {
		expect(
			ProjectManagedHarnessSetup({
				available: true,
				pending: true,
				report: installation({
					activity: "installing",
					activity_detail: "Installing Python dependencies…",
					activity_phase: "verifying",
				}),
			}),
		).toMatchObject({
			action: "none",
			busy: true,
			label: "Installing Python dependencies…",
		});
	});

	it("opens a service authorization while authentication is pending", () => {
		expect(
			ProjectManagedHarnessSetup({
				available: true,
				pending: true,
				report: installation({
					activity: "authenticating",
					authorization: {
						attempt_id: "attempt_1",
						expires_at_ms: 2_000,
						instructions: "Continue in the browser.",
						mode: "auto",
						url: "https://example.invalid/authorize",
					},
					managed: true,
				}),
			}),
		).toMatchObject({
			action: "open_authorization",
			authorization_url: "https://example.invalid/authorize",
			busy: true,
			label: "Open sign-in…",
		});
	});

	it("uses provider usage for the authenticated email", () => {
		expect(
			ProjectManagedHarnessSetup({
				available: true,
				pending: false,
				report: installation({ credentials_present: true, managed: true }),
				usage: usage({
					account_email: "reader@example.com",
					authentication: "authenticated",
				}),
			}),
		).toMatchObject({
			action: "none",
			email: "reader@example.com",
			label: "Signed in as",
			ready: true,
		});
	});

	it("keeps managed Hermes configuration provider-owned after installation", () => {
		expect(
			ProjectManagedHarnessSetup({
				available: true,
				external_auth: true,
				pending: false,
				report: installation({ engine_id: "hermes", managed: true }),
				usage: usage({
					authentication: "unauthenticated",
					engine_id: "hermes",
				}),
			}),
		).toMatchObject({ action: "open_external_setup", label: "Configure Hermes" });
	});
});
