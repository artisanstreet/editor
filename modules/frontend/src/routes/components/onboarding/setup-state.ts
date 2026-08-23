import type { EngineInstallationReport, EngineUsageReport } from "@artisan/protocol";

export type HarnessSetupAction =
	| "authenticate"
	| "install"
	| "open_authorization"
	| "open_external_setup"
	| "none";

export type HarnessSetupStatus =
	| "checking"
	| "downloading"
	| "failed"
	| "ready"
	| "sign_in"
	| "waiting_for_sign_in";

export interface HarnessSetupState {
	readonly action: HarnessSetupAction;
	readonly authorization_url?: string;
	readonly busy: boolean;
	readonly email?: string;
	readonly failure?: string;
	readonly label: string;
	readonly ready: boolean;
	readonly status: HarnessSetupStatus;
}

const installation_phase_labels: Readonly<Record<string, string>> = {
	checking: "Checking release…",
	downloading: "Downloading…",
	provisioning: "Preparing home…",
	resolving: "Resolving release…",
	staging: "Staging…",
	verifying: "Verifying…",
};

const signed_in = (usage: EngineUsageReport | undefined): HarnessSetupState => ({
	action: "none",
	busy: false,
	...(usage?.account_email === undefined ? {} : { email: usage.account_email }),
	label: usage?.account_email === undefined ? "Signed in" : "Signed in as",
	ready: true,
	status: "ready",
});

export const ProjectManagedHarnessSetup = (input: {
	readonly available: boolean;
	readonly error?: string;
	readonly external_auth?: boolean;
	readonly pending: boolean;
	readonly report?: EngineInstallationReport;
	readonly usage?: EngineUsageReport;
}): HarnessSetupState => {
	const { available, error, external_auth, pending, report, usage } = input;
	if (!available || report === undefined)
		return {
			action: "none",
			busy: !available,
			...(available ? { failure: "Installation status is unavailable." } : {}),
			label: available ? "Unavailable" : "Checking…",
			ready: false,
			status: available ? "failed" : "checking",
		};

	if (report.activity === "installing" || (pending && !report.managed))
		return {
			action: "none",
			busy: true,
			label:
				report.activity_detail ??
				installation_phase_labels[report.activity_phase ?? ""] ??
				"Installing…",
			ready: false,
			status: "downloading",
		};

	if (report.activity === "authenticating")
		return {
			action:
				report.authorization === undefined ? "none" : "open_authorization",
			busy: true,
			...(report.authorization === undefined
				? {}
				: {
						authorization_url: report.authorization.url,
					}),
			label:
				report.authorization === undefined
					? "Waiting for sign-in…"
					: "Open sign-in…",
			ready: false,
			status: "waiting_for_sign_in",
		};

	if (report.activity === "failed") {
		const installed = report.managed;
		return {
			action: installed ? "authenticate" : "install",
			busy: false,
			failure: error ?? report.failure ?? "Setup did not complete.",
			label: installed ? "Try Sign In Again" : "Retry Download",
			ready: false,
			status: "failed",
		};
	}

	if (
		report.credentials_present ||
		usage?.authentication === "authenticated"
	)
		return signed_in(usage);

	if (!report.managed)
		return {
			action: "install",
			busy: false,
			...(error === undefined ? {} : { failure: error }),
			label: error === undefined ? "Download" : "Retry Download",
			ready: false,
			status: "sign_in",
		};

	if (external_auth === true)
		return {
			action: "open_external_setup",
			busy: false,
			...(error === undefined ? {} : { failure: error }),
			label: "Configure Hermes",
			ready: false,
			status: "sign_in",
		};

	return {
		action: "authenticate",
		busy: pending,
		...(error === undefined ? {} : { failure: error }),
		label: pending ? "Starting sign-in…" : "Sign In",
		ready: false,
		status: "sign_in",
	};
};
