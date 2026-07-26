import { Effect } from "effect";

import type {
	CommandReceiptEnvelope,
	EventEnvelope,
	GlobalGuidanceDriftResolutionEnvelope,
	GlobalGuidanceRetryEnvelope,
	GlobalGuidanceSelectionEnvelope,
	GlobalGuidanceUpdateEnvelope,
	ModelBehaviourDriftResolutionEnvelope,
	ModelBehaviourRetryEnvelope,
	ModelBehaviourUpdateEnvelope,
	ProtocolErrorDetail,
} from "@artisan/protocol";

import { GuidanceFileStoreFailure } from "../../../guidance/file-store";
import { global_guidance_thread_id } from "../../../guidance/guidance-repository";
import {
	GlobalGuidanceConflict,
	GlobalGuidanceInvariantError,
	GlobalGuidanceService,
} from "../../../guidance/guidance-service";
import { model_behaviour_thread_id } from "../../../model-behaviour/model-behaviour-repository";
import {
	ModelBehaviourConflict,
	ModelBehaviourInvariantError,
	ModelBehaviourService,
} from "../../../model-behaviour/model-behaviour-service";
import {
	CommandIdConflict,
	JournalInvariantError,
	JournalStore,
	JournalStoreFailure,
} from "../../../persistence/journal-store";
import { RuntimeMetadata } from "../../../runtime/runtime-metadata";

export type SettingsMutationEnvelope =
	| GlobalGuidanceDriftResolutionEnvelope
	| GlobalGuidanceRetryEnvelope
	| GlobalGuidanceSelectionEnvelope
	| GlobalGuidanceUpdateEnvelope
	| ModelBehaviourDriftResolutionEnvelope
	| ModelBehaviourRetryEnvelope
	| ModelBehaviourUpdateEnvelope;

export interface SettingsMutationResult {
	readonly events: ReadonlyArray<EventEnvelope>;
	readonly receipt: CommandReceiptEnvelope;
}

const GuidanceErrorDetail = (error: unknown): ProtocolErrorDetail => {
	if (error instanceof CommandIdConflict) {
		return {
			code: "command.id_conflict",
			message: "This command id has already been used for different intent.",
			retryable: false,
		};
	}

	if (error instanceof GlobalGuidanceConflict) {
		return {
			code: "guidance.conflict",
			message: "The provider guidance changed; refresh before retrying.",
			retryable: false,
		};
	}

	if (error instanceof JournalInvariantError) {
		return {
			code: "journal.invariant_failed",
			message: "The journal could not reconstruct the guidance command.",
			retryable: false,
		};
	}

	if (error instanceof GlobalGuidanceInvariantError) {
		return {
			code: "guidance.invariant_failed",
			message: "The canonical guidance state failed validation.",
			retryable: false,
		};
	}

	if (error instanceof GuidanceFileStoreFailure || error instanceof JournalStoreFailure) {
		return {
			code: "guidance.unavailable",
			message: "Global guidance could not be durably updated.",
			retryable: true,
		};
	}

	return {
		code: "guidance.unavailable",
		message: "Global guidance could not be durably updated.",
		retryable: true,
	};
};

const ModelBehaviourErrorDetail = (error: unknown): ProtocolErrorDetail => {
	if (error instanceof CommandIdConflict) {
		return {
			code: "command.id_conflict",
			message: "This command id has already been used for different intent.",
			retryable: false,
		};
	}

	if (error instanceof ModelBehaviourConflict) {
		return {
			code: "model_behaviour.conflict",
			message: error.message,
			retryable: false,
		};
	}

	if (error instanceof JournalInvariantError || error instanceof ModelBehaviourInvariantError) {
		return {
			code: "model_behaviour.invariant_failed",
			message: "The canonical Model Behaviour state failed validation.",
			retryable: false,
		};
	}

	return {
		code: "model_behaviour.unavailable",
		message: "Model Behaviour settings could not be durably reconciled.",
		retryable: true,
	};
};

export const MakeSettingsMutationHandler = Effect.gen(function* () {
	const guidance = yield* GlobalGuidanceService;
	const journal = yield* JournalStore;
	const metadata = yield* RuntimeMetadata;
	const model_behaviour = yield* ModelBehaviourService;

	const Receipt = (
		envelope: SettingsMutationEnvelope,
		thread_id: string,
		payload: CommandReceiptEnvelope["payload"],
	) =>
		Effect.gen(function* () {
			const message_id = yield* metadata.MakeId("message");
			const sent_at = yield* metadata.Now;

			return {
				causation_id: envelope.message_id,
				correlation_id: envelope.message_id,
				kind: "command.receipt" as const,
				message_id,
				origin: "backend" as const,
				payload,
				protocol_version: 1 as const,
				schema_version: 1 as const,
				sent_at,
				thread_id,
			};
		});

	const GuidanceMutation = (
		envelope:
			| GlobalGuidanceDriftResolutionEnvelope
			| GlobalGuidanceRetryEnvelope
			| GlobalGuidanceSelectionEnvelope
			| GlobalGuidanceUpdateEnvelope,
	) => {
		const trace = {
			message_id: envelope.message_id,
			origin: envelope.origin,
			sent_at: envelope.sent_at,
		};
		const operation =
			envelope.kind === "guidance.update"
				? guidance.Update({ ...trace, content: envelope.payload.content })
				: envelope.kind === "guidance.selection"
					? guidance.Select({ ...trace, ...envelope.payload })
					: envelope.kind === "guidance.drift.resolve"
						? guidance.ResolveDrift({ ...trace, ...envelope.payload })
						: guidance.RetrySync({ ...trace, ...envelope.payload });

		return operation.pipe(
			Effect.flatMap(({ acceptance }) =>
				Receipt(envelope, global_guidance_thread_id, {
					journal_sequence: acceptance.event.journal_sequence,
					status: acceptance.status,
				}).pipe(
					Effect.map((receipt) => ({
						events: [acceptance.event],
						receipt,
					})),
				),
			),
			Effect.catch((error) =>
				Receipt(envelope, global_guidance_thread_id, {
					error: GuidanceErrorDetail(error),
					status: "rejected",
				}).pipe(Effect.map((receipt) => ({ events: [], receipt }))),
			),
		);
	};

	const ModelBehaviourMutation = (
		envelope:
			| ModelBehaviourDriftResolutionEnvelope
			| ModelBehaviourRetryEnvelope
			| ModelBehaviourUpdateEnvelope,
	) => {
		const trace = {
			message_id: envelope.message_id,
			origin: envelope.origin,
			sent_at: envelope.sent_at,
		};
		const operation =
			envelope.kind === "model_behaviour.update"
				? model_behaviour.Update({ ...trace, ...envelope.payload })
				: envelope.kind === "model_behaviour.drift.resolve"
					? model_behaviour.ResolveDrift({ ...trace, ...envelope.payload })
					: model_behaviour.RetrySync({ ...trace, ...envelope.payload });

		return operation.pipe(
			Effect.flatMap((result) =>
				Effect.gen(function* () {
					const events = yield* journal.ReadCorrelatedEvents(envelope.message_id);
					const journal_sequence =
						events.at(-1)?.journal_sequence ?? (yield* journal.ReadWatermark());
					const receipt = yield* Receipt(envelope, model_behaviour_thread_id, {
						journal_sequence,
						status: result.status,
					});

					return { events, receipt };
				}),
			),
			Effect.catch((error) =>
				Receipt(envelope, model_behaviour_thread_id, {
					error: ModelBehaviourErrorDetail(error),
					status: "rejected",
				}).pipe(Effect.map((receipt) => ({ events: [], receipt }))),
			),
		);
	};

	return (envelope: SettingsMutationEnvelope): Effect.Effect<SettingsMutationResult> => {
		switch (envelope.kind) {
			case "guidance.drift.resolve":
			case "guidance.selection":
			case "guidance.sync.retry":
			case "guidance.update":
				return GuidanceMutation(envelope);
			case "model_behaviour.drift.resolve":
			case "model_behaviour.sync.retry":
			case "model_behaviour.update":
				return ModelBehaviourMutation(envelope);
		}
	};
});
