import { Context, Data, Effect, Option } from "effect";

import type { EngineGlobalGuidance } from "@artisan/engines";
import type {
	GlobalGuidanceDriftResolutionRequest,
	GlobalGuidanceProvider,
	GlobalGuidanceRetryRequest,
	GlobalGuidanceSelectionRequest,
	GlobalGuidanceSnapshot,
} from "@artisan/protocol";

import type { GuidanceFileStoreFailure } from "./file-store";
import type { GlobalGuidanceAcceptance } from "./repository";
import type { JournalStoreError } from "../persistence/journal-store";

/** Configures the one canonical file and its recoverable backup directory. */
export interface GlobalGuidanceServiceOptions {
	readonly backups_directory: string;
	readonly canonical_path: string;
}

/** Supplies trace identity for a durable user-initiated guidance operation. */
export interface GlobalGuidanceMutationTrace {
	readonly message_id: string;
	readonly origin: "frontend";
	readonly sent_at: string;
}

/** Returns the durable operation result together with the refreshed projection. */
export interface GlobalGuidanceMutationResult {
	readonly acceptance: GlobalGuidanceAcceptance;
	readonly snapshot: GlobalGuidanceSnapshot;
}

/** Rejects a stale selection or drift action after provider files changed. */
export class GlobalGuidanceConflict extends Data.TaggedError("GlobalGuidanceConflict")<{
	readonly provider: GlobalGuidanceProvider;
	readonly reason:
		| "candidate_changed"
		| "drift_changed"
		| "provider_unavailable"
		| "selection_not_required";
}> {}

/** Reports a canonical-file invariant or a failed post-write verification. */
export class GlobalGuidanceInvariantError extends Data.TaggedError("GlobalGuidanceInvariantError")<{
	readonly operation: string;
}> {}

export type GlobalGuidanceServiceError =
	| GlobalGuidanceConflict
	| GlobalGuidanceInvariantError
	| GuidanceFileStoreFailure
	| JournalStoreError;

/** Owns first-run import, canonical writes, provider sync, drift, and runtime handoff. */
export class GlobalGuidanceService extends Context.Service<
	GlobalGuidanceService,
	{
		readonly Get: Effect.Effect<GlobalGuidanceSnapshot, GlobalGuidanceServiceError>;
		readonly Initialize: Effect.Effect<GlobalGuidanceSnapshot, GlobalGuidanceServiceError>;
		readonly ResolveDrift: (
			input: GlobalGuidanceDriftResolutionRequest & GlobalGuidanceMutationTrace,
		) => Effect.Effect<GlobalGuidanceMutationResult, GlobalGuidanceServiceError>;
		readonly ResolveForEngine: (
			engine_id: string,
		) => Effect.Effect<Option.Option<EngineGlobalGuidance>, GlobalGuidanceServiceError>;
		readonly RetrySync: (
			input: GlobalGuidanceRetryRequest & GlobalGuidanceMutationTrace,
		) => Effect.Effect<GlobalGuidanceMutationResult, GlobalGuidanceServiceError>;
		readonly Select: (
			input: GlobalGuidanceSelectionRequest & GlobalGuidanceMutationTrace,
		) => Effect.Effect<GlobalGuidanceMutationResult, GlobalGuidanceServiceError>;
		readonly Update: (
			input: GlobalGuidanceMutationTrace & { readonly content: string },
		) => Effect.Effect<GlobalGuidanceMutationResult, GlobalGuidanceServiceError>;
	}
>()("Artisan/GlobalGuidanceService") {}
