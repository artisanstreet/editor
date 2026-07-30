import { Context, Effect } from "effect";

import type { CommandEnvelope, TerminalSession } from "@artisan/protocol";

import type {
	StoredTerminalSession,
	TerminalCommandClaim,
	TerminalCommandTransition,
	TerminalCommit,
	TerminalLifecycleAction,
	TerminalRepositoryError,
} from "./model";

/** Persists generation-bound terminal claims and atomic lifecycle commits. */
export class TerminalRepository extends Context.Service<
	TerminalRepository,
	{
		readonly Claim: (
			command: CommandEnvelope,
			instance_id: string,
		) => Effect.Effect<TerminalCommandClaim, TerminalRepositoryError>;
		readonly CommitAmbiguous: (
			command: CommandEnvelope,
			claim: TerminalCommandClaim,
			failure: string,
		) => Effect.Effect<TerminalCommit, TerminalRepositoryError>;
		readonly CommitCommand: (
			command: CommandEnvelope,
			generation: number,
			action: TerminalLifecycleAction,
			transition: TerminalCommandTransition,
		) => Effect.Effect<TerminalCommit, TerminalRepositoryError>;
		readonly CommitExit: (
			terminal_id: string,
			generation: number,
			exit: {
				readonly exit_code: number | null;
				readonly reason: "closed" | "exited" | "killed" | "output_overflow";
				readonly signal: number | null;
			},
			action: TerminalLifecycleAction,
		) => Effect.Effect<TerminalCommit, TerminalRepositoryError>;
		readonly CommitRecovery: (
			terminal_id: string,
			generation: number,
			failure: string,
		) => Effect.Effect<TerminalCommit, TerminalRepositoryError>;
		readonly CompleteCommand: (
			message_id: string,
			generation: number,
			status: "completed" | "failed",
			journal_sequence: number,
			failure?: string,
		) => Effect.Effect<void, TerminalRepositoryError>;
		readonly List: (
			thread_id: string,
			workspace_id: string,
		) => Effect.Effect<ReadonlyArray<TerminalSession>, TerminalRepositoryError>;
		readonly ReadOwned: (
			terminal_id: string,
			thread_id: string,
		) => Effect.Effect<StoredTerminalSession, TerminalRepositoryError>;
		readonly ReadStale: (
			instance_id: string,
		) => Effect.Effect<ReadonlyArray<StoredTerminalSession>, TerminalRepositoryError>;
	}
>()("Artisan/TerminalRepository") {}
