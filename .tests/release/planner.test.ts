import { describe, expect, it } from "vitest";

import { make_release_plan } from "../../build/release/planner.ts";

const commit = "a".repeat(40);

describe("release planner", () => {
	it("keeps pull request staging fast and non-publishing", () => {
		const plan = make_release_plan({
			event: "pull_request",
			ref: "refs/pull/1/merge",
			commit,
			version: "0.1.0",
			workspace_version: "0.1.0",
			run_id: "12",
		});
		expect(plan.construct_candidate).toBe(false);
		expect(plan.publish).toBe(false);
	});

	it("binds a candidate to version, commit, and run", () => {
		const plan = make_release_plan({
			event: "workflow_dispatch",
			ref: "refs/heads/candidate",
			commit,
			version: "0.1.0-beta.1",
			workspace_version: "0.1.0-beta.1",
			run_id: "42",
			mode: "release",
		});
		expect(plan.candidate_artifact).toBe(`artisan-candidate-0.1.0-beta.1-${commit}-42`);
		expect(plan.assets).toEqual([
			"artisan-0.1.0-beta.1-windows-x64.zip",
			"artisan-bootstrap-windows-x64.exe",
			"artisan-bootstrap-windows-x64.exe.sha256",
			"release-manifest.json",
			"release-manifest.sig",
		]);
	});

	it("resumes only an exact prior identity", () => {
		expect(() =>
			make_release_plan({
				event: "workflow_dispatch",
				ref: "refs/heads/candidate",
				commit,
				version: "0.1.0",
				workspace_version: "0.1.0",
				run_id: "99",
				mode: "resume",
				resume_commit: commit,
				resume_version: "0.1.0",
			}),
		).toThrow("Resume requires exact version, commit, and run ID.");
	});

	it("rejects manual release work outside the protected pointer", () => {
		expect(() =>
			make_release_plan({
				event: "workflow_dispatch",
				ref: "refs/heads/master",
				commit,
				version: "0.1.0",
				workspace_version: "0.1.0",
				run_id: "99",
				mode: "dry-run",
			}),
		).toThrow("protected candidate branch");
	});

	it("rejects version drift from the Rust workspace", () => {
		expect(() =>
			make_release_plan({
				event: "workflow_dispatch",
				ref: "refs/heads/candidate",
				commit,
				version: "0.2.0",
				workspace_version: "0.1.0",
				run_id: "99",
				mode: "release",
			}),
		).toThrow("does not match workspace version");
	});
});
