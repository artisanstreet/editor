import type {
	ProjectAffinityEvidenceKind,
	ProjectAffinityScore,
	ProjectRef,
} from "@artisan/protocol";

/** Holds one transparent weight and per-project cap for a durable evidence kind. */
export interface ProjectAffinityWeight {
	readonly cap: number;
	readonly weight: number;
}

/** Defines the opinionated V1 score contribution for each canonical evidence kind. */
export const ProjectAffinityWeights: Readonly<
	Record<ProjectAffinityEvidenceKind, ProjectAffinityWeight>
> = {
	active_working_directory: { cap: 36, weight: 18 },
	file_artifact: { cap: 30, weight: 15 },
	file_mutation: { cap: 48, weight: 24 },
	git_root: { cap: 40, weight: 40 },
	historical_working_directory: { cap: 18, weight: 6 },
	process_owner: { cap: 24, weight: 12 },
	project_mention: { cap: 20, weight: 10 },
	terminal_working_directory: { cap: 24, weight: 12 },
	thread_metadata: { cap: 8, weight: 4 },
};

/** Defines the confidence and lead required for moves, suggestions, and links. */
export const ProjectAffinityThresholds = {
	automatic_lead: 25,
	automatic_score: 70,
	linked_maximum: 3,
	linked_score: 25,
	linked_winner_distance: 35,
	suggestion_lead: 12,
	suggestion_score: 45,
} as const;

/** Bounds active evidence globally so recent sustained work can replace an old owner. */
export const ProjectAffinityRecencyLimits: Partial<
	Readonly<Record<ProjectAffinityEvidenceKind, number>>
> = {
	active_working_directory: 4,
	file_artifact: 8,
	file_mutation: 8,
	git_root: 4,
	process_owner: 4,
	terminal_working_directory: 4,
};

/** Supplies one unique content-free evidence fact to the scoring policy. */
export interface ProjectAffinityEvidence {
	readonly kind: ProjectAffinityEvidenceKind;
	readonly project: ProjectRef;
	readonly source_journal_sequence?: number;
}

/** Describes the deterministic result of evaluating one thread's evidence. */
export interface ProjectAffinityDecision {
	readonly linked_projects: ReadonlyArray<ProjectRef>;
	readonly primary_project?: ProjectRef;
	readonly rehome_suggestion?: {
		readonly project: ProjectRef;
		readonly score: number;
	};
	readonly scores: ReadonlyArray<ProjectAffinityScore>;
}

const high_integrity_evidence = new Set<ProjectAffinityEvidenceKind>([
	"active_working_directory",
	"file_mutation",
	"git_root",
]);

function contributing_evidence(evidence: ReadonlyArray<ProjectAffinityEvidence>) {
	const indexed = evidence.map((item, index) => ({
		index,
		item,
		sequence: item.source_journal_sequence ?? index + 1,
	}));
	const selected = new Set<number>();
	const selected_by_kind = new Map<ProjectAffinityEvidenceKind, number>();

	for (const candidate of [...indexed].sort(
		(left, right) => right.sequence - left.sequence || right.index - left.index,
	)) {
		const limit = ProjectAffinityRecencyLimits[candidate.item.kind];
		const count = selected_by_kind.get(candidate.item.kind) ?? 0;

		if (limit !== undefined && count >= limit) {
			continue;
		}

		selected.add(candidate.index);
		selected_by_kind.set(candidate.item.kind, count + 1);
	}

	return indexed.filter(({ index }) => selected.has(index)).map(({ item }) => item);
}

/** Calculates bounded scores and selects only well-separated moves or suggestions. */
export function decide_project_affinity(
	evidence: ReadonlyArray<ProjectAffinityEvidence>,
): ProjectAffinityDecision {
	const projects = new Map<
		string,
		{
			counts: Map<ProjectAffinityEvidenceKind, number>;
			has_high_integrity_evidence: boolean;
			project: ProjectRef;
		}
	>();

	for (const item of contributing_evidence(evidence)) {
		const current = projects.get(item.project.project_id) ?? {
			counts: new Map<ProjectAffinityEvidenceKind, number>(),
			has_high_integrity_evidence: false,
			project: item.project,
		};

		current.counts.set(item.kind, (current.counts.get(item.kind) ?? 0) + 1);
		current.has_high_integrity_evidence ||= high_integrity_evidence.has(item.kind);
		projects.set(item.project.project_id, current);
	}

	const scored = [...projects.values()]
		.map(({ counts, has_high_integrity_evidence, project }) => {
			const evidence_counts = [...counts.entries()]
				.sort(([left], [right]) => left.localeCompare(right))
				.map(([kind, count]) => ({ count, kind }));
			const score = Math.min(
				100,
				evidence_counts.reduce((total, item) => {
					const rule = ProjectAffinityWeights[item.kind];

					return total + Math.min(item.count * rule.weight, rule.cap);
				}, 0),
			);

			return {
				has_high_integrity_evidence,
				projection: { evidence: evidence_counts, project, score },
			};
		})
		.sort(
			(left, right) =>
				right.projection.score - left.projection.score ||
				left.projection.project.project_id.localeCompare(
					right.projection.project.project_id,
				),
		);
	const [winner, runner_up] = scored;
	const lead = winner ? winner.projection.score - (runner_up?.projection.score ?? 0) : 0;
	const primary_project =
		winner &&
		winner.projection.score >= ProjectAffinityThresholds.automatic_score &&
		lead >= ProjectAffinityThresholds.automatic_lead &&
		winner.has_high_integrity_evidence
			? winner.projection.project
			: undefined;
	const rehome_suggestion =
		winner &&
		primary_project === undefined &&
		winner.projection.score >= ProjectAffinityThresholds.suggestion_score &&
		lead >= ProjectAffinityThresholds.suggestion_lead
			? {
					project: winner.projection.project,
					score: winner.projection.score,
				}
			: undefined;
	const linked_projects = winner
		? scored
				.filter(
					(candidate) =>
						candidate.projection.project.project_id !== primary_project?.project_id &&
						candidate.projection.score >= ProjectAffinityThresholds.linked_score &&
						winner.projection.score - candidate.projection.score <=
							ProjectAffinityThresholds.linked_winner_distance,
				)
				.slice(0, ProjectAffinityThresholds.linked_maximum)
				.map((candidate) => candidate.projection.project)
		: [];

	return {
		linked_projects,
		...(primary_project === undefined ? {} : { primary_project }),
		...(rehome_suggestion === undefined ? {} : { rehome_suggestion }),
		scores: scored.map((candidate) => candidate.projection),
	};
}
