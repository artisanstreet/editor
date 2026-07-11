import { describe, expect, it } from "vitest";

import type { ProjectAffinityEvidenceKind, ProjectRef } from "@artisan/protocol";

import {
	decide_project_affinity,
	ProjectAffinityWeights,
	type ProjectAffinityEvidence,
} from "../../modules/backend/src/threads/project-affinity-policy";

const ProjectAlpha: ProjectRef = {
	display_name: "Alpha",
	project_id: "project_alpha",
	root_path: "C:/work/alpha",
};

const ProjectBeta: ProjectRef = {
	display_name: "Beta",
	project_id: "project_beta",
	root_path: "C:/work/beta",
};

function evidence(
	project: ProjectRef,
	...kinds: ReadonlyArray<ProjectAffinityEvidenceKind>
): ReadonlyArray<ProjectAffinityEvidence> {
	return kinds.map((kind) => ({ kind, project }));
}

describe("project affinity policy", () => {
	it("selects a clear high-integrity owner only above the automatic threshold", () => {
		const decision = decide_project_affinity([
			...evidence(ProjectAlpha, "git_root", "file_mutation", "active_working_directory"),
			...evidence(ProjectBeta, "project_mention", "thread_metadata"),
		]);

		expect(decision.primary_project).toEqual(ProjectAlpha);
		expect(decision.rehome_suggestion).toBeUndefined();
		expect(decision.scores[0]).toMatchObject({ project: ProjectAlpha, score: 82 });
	});

	it("creates a medium-confidence suggestion without changing the primary project", () => {
		const decision = decide_project_affinity([
			...evidence(
				ProjectAlpha,
				"file_mutation",
				"project_mention",
				"terminal_working_directory",
				"thread_metadata",
			),
		]);

		expect(decision.primary_project).toBeUndefined();
		expect(decision.rehome_suggestion).toEqual({
			project: ProjectAlpha,
			score: 50,
		});
	});

	it("treats worktree and diff ownership as high-integrity Git evidence", () => {
		const decision = decide_project_affinity([
			...evidence(ProjectAlpha, "git_branch", "project_mention"),
			...evidence(ProjectBeta, "git_worktree", "git_worktree", "git_diff", "process_owner"),
		]);

		expect(decision.primary_project).toEqual(ProjectBeta);
		expect(decision.scores[0]).toMatchObject({
			project: ProjectBeta,
			score: 80,
		});
	});

	it("requires separate high-integrity events before automatically rehoming", () => {
		const decision = decide_project_affinity(
			evidence(ProjectBeta, "git_root", "git_worktree", "git_branch", "git_diff").map(
				(item) => ({ ...item, source_journal_sequence: 42 }),
			),
		);

		expect(decision.primary_project).toBeUndefined();
		expect(decision.rehome_suggestion).toEqual({
			project: ProjectBeta,
			score: 92,
		});
	});

	it("keeps close multi-repository candidates linked instead of forcing a winner", () => {
		const decision = decide_project_affinity([
			...evidence(ProjectAlpha, "git_root", "terminal_working_directory", "thread_metadata"),
			...evidence(
				ProjectBeta,
				"file_mutation",
				"terminal_working_directory",
				"project_mention",
			),
		]);

		expect(decision.primary_project).toBeUndefined();
		expect(decision.rehome_suggestion).toBeUndefined();
		expect(decision.linked_projects).toEqual([ProjectAlpha, ProjectBeta]);
	});

	it("ages active ownership toward sustained recent work without losing deterministic scoring", () => {
		const decision = decide_project_affinity([
			...evidence(ProjectAlpha, "active_working_directory", "git_root").map(
				(item, index) => ({ ...item, source_journal_sequence: index + 1 }),
			),
			...Array.from({ length: 4 }, (_, index) => [
				{
					kind: "active_working_directory" as const,
					project: ProjectBeta,
					source_journal_sequence: index + 3,
				},
				{
					kind: "git_root" as const,
					project: ProjectBeta,
					source_journal_sequence: index + 3,
				},
			]).flat(),
		]);

		expect(decision.primary_project).toEqual(ProjectBeta);
		expect(decision.scores[0]).toMatchObject({ project: ProjectBeta, score: 76 });
		expect(decision.scores).toHaveLength(1);
	});

	it("caps repeated weak evidence and exposes the contributing count", () => {
		const repeated_mentions = Array.from({ length: 8 }, () => "project_mention" as const);
		const decision = decide_project_affinity(evidence(ProjectAlpha, ...repeated_mentions));
		const [score] = decision.scores;

		expect(ProjectAffinityWeights.project_mention).toEqual({ cap: 20, weight: 10 });
		expect(score).toMatchObject({
			evidence: [{ count: 8, kind: "project_mention" }],
			score: 20,
		});
	});

	it("bounds scores at 100 and breaks exact ties by stable project identity", () => {
		const saturated = Array.from(
			{ length: 10 },
			() =>
				[
					...evidence(
						ProjectBeta,
						"git_root",
						"file_mutation",
						"active_working_directory",
					),
					...evidence(
						ProjectAlpha,
						"git_root",
						"file_mutation",
						"active_working_directory",
					),
				] as const,
		).flat();
		const decision = decide_project_affinity(saturated);

		expect(decision.scores.map((score) => score.score)).toEqual([100, 100]);
		expect(decision.scores.map((score) => score.project.project_id)).toEqual([
			"project_alpha",
			"project_beta",
		]);
		expect(decision.primary_project).toBeUndefined();
	});
});
