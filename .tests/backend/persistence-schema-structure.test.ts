import { readdirSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

import * as PersistenceSchema from "../../modules/backend/src/persistence/tables";

const schema_facade = resolve("modules/backend/src/persistence/tables.ts");
const schema_modules = resolve("modules/backend/src/persistence/schema");
const line_count = (path: string) => readFileSync(path, "utf8").split(/\r?\n/u).length;

const expected_tables = [
	"AgentInstances",
	"AgentRuns",
	"ArtisanToolApprovals",
	"ArtisanToolInvocations",
	"Assignments",
	"ConversationItems",
	"ConversationPatches",
	"ConversationSources",
	"ConversationThreads",
	"ConversationTurns",
	"DisabledEngines",
	"EventStreams",
	"GitMutationOperations",
	"GitWorkspaceProjections",
	"GlobalGuidanceCanonical",
	"GlobalGuidanceProviderSync",
	"JournalCommands",
	"JournalEvents",
	"LegacyWorkspaceChangeProjections",
	"MarketplaceCapabilities",
	"MarketplaceCapabilityArtifacts",
	"MarketplaceCapabilityMirrors",
	"MarketplaceCapabilityOperations",
	"MarketplaceRoutineMirrors",
	"MarketplaceRoutineOperations",
	"MarketplaceRoutines",
	"MessageImageAttachments",
	"ModelBehaviourProviderStates",
	"ModelBehaviourSettings",
	"ModelFavorites",
	"NativeSubagentBindings",
	"NativeSubagentObservationInbox",
	"NativeSubagentTranscriptInbox",
	"OrchestrationArtifacts",
	"OrchestrationCoordinators",
	"OrchestrationGraphCommands",
	"OrchestrationGraphEdges",
	"OrchestrationGroups",
	"OrchestrationIntake",
	"OrchestrationInteractions",
	"OrchestrationJoins",
	"OrchestrationMessages",
	"OrchestrationOutbox",
	"OrchestrationRawObservations",
	"OrchestrationRuns",
	"PreviewCommands",
	"PreviewDispatchLeases",
	"PreviewInspectionSessions",
	"PreviewTargets",
	"ProjectIdentities",
	"ProjectionRebuildLocks",
	"Projects",
	"SessionDefaults",
	"SessionModelDefaults",
	"SurfaceItems",
	"SurfaceUsageTotals",
	"TerminalCommands",
	"TerminalSessions",
	"ThreadErasureClaims",
	"ThreadProjectAffinityEvidence",
	"ThreadRetentionPolicies",
	"ThreadTombstones",
	"Threads",
	"WorkspaceChangeDiffs",
	"WorkspaceChangeOperations",
	"WorkspaceChangeSnapshots",
	"WorkspaceChanges",
	"WorkspaceConflicts",
	"WorkspaceMutationAuthorities",
	"WorkspaceMutationPayloads",
] as const;

describe("persistence schema structure", () => {
	it("keeps the compatibility facade thin and every domain module bounded", () => {
		expect(line_count(schema_facade)).toBeLessThan(100);

		for (const file_name of readdirSync(schema_modules).filter((name) =>
			name.endsWith(".ts"),
		)) {
			expect(line_count(resolve(schema_modules, file_name)), file_name).toBeLessThan(800);
		}
	});

	it("preserves every public table export through the facade", () => {
		expect(Object.keys(PersistenceSchema).toSorted()).toEqual([...expected_tables].toSorted());
	});
});
