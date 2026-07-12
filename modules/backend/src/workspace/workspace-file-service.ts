import { Context, Crypto, Data, Effect, Encoding, Layer, Result, Schema } from "effect";

import {
	ContentIdentity,
	Identifier,
	IsoDateTime,
	RawOrigin,
	WorkspaceFileReadQuery,
	WorkspaceFileReplaceRequest,
	workspace_text_maximum_bytes,
	type WorkspaceFileReadQueryResult,
	type WorkspaceFileReadQuery as WorkspaceFileReadQueryValue,
} from "@artisan/protocol";

import { WorkspaceBoundedRegularFileStoreRegistry } from "../filesystem/workspace-bounded-regular-file-store-registry";
import {
	type WorkspaceChangeCommit,
	WorkspaceChangeRepository,
} from "./workspace-change-repository";
import {
	ComputeContentIdentity,
	DecodeWorkspaceText,
	EncodeWorkspaceText,
} from "./workspace-file-content";
import {
	WorkspaceMutationAuthority,
	WorkspaceMutationAuthorityConflict,
} from "./workspace-mutation-authority";
import {
	WorkspaceMutationPayloadStore,
	WorkspaceMutationPayloadStoreUnavailable,
} from "./workspace-mutation-payload-store";
import { WorkspaceSnapshotStore } from "./workspace-snapshot-store";
import { WorkspaceEvidenceRecorder } from "./workspace-evidence-recorder";

const WorkspaceFileReplaceInput = Schema.Struct({
	...WorkspaceFileReplaceRequest.fields,
	agent_id: Identifier,
	message_id: Identifier,
	raw_origin: Schema.optional(RawOrigin),
	run_id: Identifier,
	sent_at: IsoDateTime,
	thread_id: Identifier,
});

/** Carries the envelope attribution and replacement request accepted by the controlled workspace service. */
export type WorkspaceFileReplaceInput = typeof WorkspaceFileReplaceInput.Type;

/** Reports a source-free controlled workspace file failure. */
export class WorkspaceFileServiceError extends Data.TaggedError("WorkspaceFileServiceError")<{
	readonly operation: "read" | "replace";
	readonly reason: "changed" | "failed";
}> {}

/** Owns controlled UTF-8 workspace reads and recoverable attributed replacements. */
export class WorkspaceFileService extends Context.Service<
	WorkspaceFileService,
	{
		readonly Read: (
			query: WorkspaceFileReadQueryValue,
		) => Effect.Effect<WorkspaceFileReadQueryResult, WorkspaceFileServiceError>;
		readonly Replace: (
			input: WorkspaceFileReplaceInput,
		) => Effect.Effect<WorkspaceChangeCommit, WorkspaceFileServiceError>;
	}
>()("Artisan/WorkspaceFileService") {}

function failed(operation: WorkspaceFileServiceError["operation"]) {
	return new WorkspaceFileServiceError({ operation, reason: "failed" });
}

function changed() {
	return new WorkspaceFileServiceError({ operation: "replace", reason: "changed" });
}

function identities_match(left: typeof ContentIdentity.Type, right: typeof ContentIdentity.Type) {
	return (
		left.algorithm === right.algorithm &&
		left.byte_count === right.byte_count &&
		left.content_hash === right.content_hash
	);
}

function claim_fingerprint(
	input: WorkspaceFileReplaceInput,
	intended_after: typeof ContentIdentity.Type,
) {
	return JSON.stringify({
		agent_id: input.agent_id,
		change_id: input.change_id,
		expected_before: input.expected_before,
		intended_after,
		message_id: input.message_id,
		path: input.path,
		raw_origin: input.raw_origin,
		run_id: input.run_id,
		sent_at: input.sent_at,
		thread_id: input.thread_id,
		workspace_id: input.workspace_id,
	});
}

/** Builds the controlled workspace file service from its durable and filesystem capabilities. */
export const WorkspaceFileServiceLive = Layer.effect(
	WorkspaceFileService,
	Effect.gen(function* () {
		const authority = yield* WorkspaceMutationAuthority;
		const crypto = yield* Crypto.Crypto;
		const evidence = yield* WorkspaceEvidenceRecorder;
		const payloads = yield* WorkspaceMutationPayloadStore;
		const registry = yield* WorkspaceBoundedRegularFileStoreRegistry;
		const repository = yield* WorkspaceChangeRepository;
		const snapshots = yield* WorkspaceSnapshotStore;
		const ComputeIdentity = (bytes: Uint8Array) =>
			ComputeContentIdentity(bytes).pipe(Effect.provideService(Crypto.Crypto, crypto));
		const ComputeFingerprint = (
			input: WorkspaceFileReplaceInput,
			intended_after: typeof ContentIdentity.Type,
		) =>
			Effect.gen(function* () {
				const bytes = new TextEncoder().encode(claim_fingerprint(input, intended_after));
				const digest = yield* crypto.digest("SHA-256", bytes);

				return Encoding.encodeHex(digest);
			});

		const Read = (query: WorkspaceFileReadQueryValue) =>
			Schema.decodeUnknownEffect(WorkspaceFileReadQuery, { onExcessProperty: "error" })(
				query,
			).pipe(
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const { reader } = yield* registry.Get(decoded.workspace_id);
						const bytes = yield* reader.ReadRegularFile(
							decoded.path,
							workspace_text_maximum_bytes,
						);
						const content = yield* DecodeWorkspaceText(bytes);
						const identity = yield* ComputeIdentity(bytes);

						return {
							content,
							identity,
							path: decoded.path,
							workspace_id: decoded.workspace_id,
						};
					}),
				),
				Effect.mapError(() => failed("read")),
			);

		const SettleRejected = (
			input: WorkspaceFileReplaceInput,
			intended_after: typeof ContentIdentity.Type,
		) =>
			Effect.gen(function* () {
				yield* snapshots.DiscardRejectedReplace({
					change_id: input.change_id,
					expected_identity: input.expected_before,
					replace_message_id: input.message_id,
					thread_id: input.thread_id,
				});
				yield* payloads.Consume({
					action: "replace",
					expected_identity: input.expected_before,
					message_id: input.message_id,
					replacement_identity: intended_after,
					thread_id: input.thread_id,
				});
			});

		const Replace = (input: WorkspaceFileReplaceInput) =>
			Schema.decodeUnknownEffect(WorkspaceFileReplaceInput, { onExcessProperty: "error" })(
				input,
			).pipe(
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const replacement = yield* EncodeWorkspaceText(decoded.content);
						const intended_after = yield* ComputeIdentity(replacement);
						const request_fingerprint = yield* ComputeFingerprint(
							decoded,
							intended_after,
						);
						const admission_result = yield* authority
							.ClaimReplace({
								_tag: "replace",
								agent_id: decoded.agent_id,
								change_id: decoded.change_id,
								expected_before: decoded.expected_before,
								intended_after,
								message_id: decoded.message_id,
								path: decoded.path,
								...(decoded.raw_origin === undefined
									? {}
									: { raw_origin: decoded.raw_origin }),
								request_fingerprint,
								run_id: decoded.run_id,
								sent_at: decoded.sent_at,
								thread_id: decoded.thread_id,
								workspace_id: decoded.workspace_id,
							})
							.pipe(Effect.result);

						if (Result.isFailure(admission_result)) {
							const error = admission_result.failure;

							if (
								error instanceof WorkspaceMutationAuthorityConflict &&
								error.reason === "operation_rejected"
							) {
								yield* SettleRejected(decoded, intended_after);

								return yield* Effect.fail(changed());
							}

							return yield* Effect.fail(error);
						}

						const admission = admission_result.success;
						const payload_input = {
							action: "replace" as const,
							expected_identity: decoded.expected_before,
							message_id: decoded.message_id,
							replacement_identity: intended_after,
							thread_id: decoded.thread_id,
						};
						const Stage = (expected: Uint8Array) =>
							Effect.gen(function* () {
								yield* payloads.Stage({ ...payload_input, expected, replacement });
								yield* snapshots.Stage({
									change_id: decoded.change_id,
									content: expected,
									expected_identity: decoded.expected_before,
									thread_id: decoded.thread_id,
								});
							});
						const ValidateAndStage = () =>
							Effect.gen(function* () {
								const current = yield* admission.store.ReadRegularFile(
									decoded.path,
									workspace_text_maximum_bytes,
								);
								yield* DecodeWorkspaceText(current);
								const current_identity = yield* ComputeIdentity(current);

								if (!identities_match(current_identity, decoded.expected_before)) {
									yield* repository.RejectChanged(decoded.message_id);
									yield* SettleRejected(decoded, intended_after);

									return yield* Effect.fail(changed());
								}

								yield* Stage(current);
							});
						const ResumeOrStage = () =>
							Effect.gen(function* () {
								return yield* payloads.Resume(payload_input).pipe(
									Effect.matchEffect({
										onFailure: (error) =>
											error instanceof
											WorkspaceMutationPayloadStoreUnavailable
												? Effect.gen(function* () {
														const has_record =
															yield* payloads.HasRecord(
																payload_input,
															);

														if (has_record) {
															return yield* Effect.fail(error);
														}

														yield* ValidateAndStage();

														return yield* payloads.Resume(
															payload_input,
														);
													})
												: Effect.fail(error),
										onSuccess: (payload) => Effect.succeed(payload),
									}),
								);
							});
						const FinalizeCommit = (payload: {
							readonly expected: Uint8Array;
							readonly replacement: Uint8Array;
						}) =>
							Effect.gen(function* () {
								yield* admission.store.FinalizeRegularFileReplacement({
									expected: payload.expected,
									maximum_bytes: workspace_text_maximum_bytes,
									operation_id: decoded.message_id,
									path: decoded.path,
									replacement: payload.replacement,
								});
								const commit = yield* repository.CommitRecorded(decoded.message_id);

								yield* evidence.RecordFilesystemMutation({
									agent_id: decoded.agent_id,
									operation: "write",
									operation_id: decoded.message_id,
									path: decoded.path,
									...(decoded.raw_origin === undefined
										? {}
										: { raw_origin: decoded.raw_origin }),
									run_id: decoded.run_id,
									thread_id: decoded.thread_id,
								});
								yield* repository.MarkEvidenceRecorded(decoded.message_id);
								yield* payloads.Consume(payload_input);

								return commit;
							});

						if (admission.claim._tag === "duplicate") {
							yield* evidence.RecordFilesystemMutation({
								agent_id: decoded.agent_id,
								operation: "write",
								operation_id: decoded.message_id,
								path: decoded.path,
								...(decoded.raw_origin === undefined
									? {}
									: { raw_origin: decoded.raw_origin }),
								run_id: decoded.run_id,
								thread_id: decoded.thread_id,
							});
							yield* repository.MarkEvidenceRecorded(decoded.message_id);
							yield* payloads.Consume(payload_input);

							return { event: admission.claim.event, status: "duplicate" as const };
						}

						if (admission.claim.operation.lifecycle === "applied") {
							const payload = yield* payloads.Resume(payload_input);

							return yield* FinalizeCommit(payload);
						}

						let payload;

						if (admission.claim._tag === "claimed") {
							yield* ValidateAndStage();
							payload = yield* payloads.Resume(payload_input);
						} else {
							payload = yield* ResumeOrStage();
							yield* snapshots.Stage({
								change_id: decoded.change_id,
								content: payload.expected,
								expected_identity: decoded.expected_before,
								thread_id: decoded.thread_id,
							});
						}
						const replace = yield* admission.store.ReplaceRegularFile({
							expected: payload.expected,
							maximum_bytes: workspace_text_maximum_bytes,
							operation_id: decoded.message_id,
							path: decoded.path,
							replacement: payload.replacement,
						});

						if (replace._tag === "Changed") {
							yield* repository.RejectChanged(decoded.message_id);
							yield* SettleRejected(decoded, intended_after);

							return yield* Effect.fail(changed());
						}

						yield* repository.MarkApplied({
							_tag: "replace",
							message_id: decoded.message_id,
							result_identity: intended_after,
						});

						return yield* FinalizeCommit(payload);
					}),
				),
				Effect.mapError((error) =>
					error instanceof WorkspaceFileServiceError ? error : failed("replace"),
				),
			);

		return { Read, Replace };
	}),
);
