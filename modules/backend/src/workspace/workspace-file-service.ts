import {
	Context,
	Crypto,
	Data,
	Effect,
	Encoding,
	Layer,
	Match,
	Option,
	Result,
	Schema,
	pipe,
} from "effect";

import {
	ContentIdentity,
	Identifier,
	IsoDateTime,
	RawOrigin,
	WorkspaceChangeReviewRequest,
	WorkspaceChangeRollbackRequest,
	WorkspaceFileReadQuery,
	WorkspaceFileReplaceRequest,
	workspace_text_maximum_bytes,
	type WorkspaceFileReadQueryResult,
	type WorkspaceFileReadQuery as WorkspaceFileReadQueryValue,
} from "@artisan/protocol";

import { WorkspaceBoundedRegularFileStoreRegistry } from "../filesystem/workspace-bounded-regular-file-store-registry";
import {
	type WorkspaceChangeCommit,
	type WorkspaceChangeReconciliation,
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
import {
	WorkspaceChangeDiffLimit,
	WorkspaceChangeDiffService,
	type PreparedWorkspaceChangeDiff,
} from "./workspace-change-diff-service";
import {
	WorkspaceReplaceApprovalConflict,
	WorkspaceReplaceApprovalInvariant,
	WorkspaceReplaceApprovalRepository,
	type WorkspaceReplaceApprovalAcceptance,
} from "./workspace-replace-approval-repository";

const default_workspace_replace_approval_reason =
	"Approve this controlled workspace file replacement.";

const WorkspaceFileReplaceInput = Schema.Struct({
	...WorkspaceFileReplaceRequest.fields,
	agent_id: Identifier,
	message_id: Identifier,
	raw_origin: Schema.optional(RawOrigin),
	run_id: Identifier,
	sent_at: IsoDateTime,
	thread_id: Identifier,
});

const WorkspaceFileReviewInput = Schema.Struct({
	...WorkspaceChangeReviewRequest.fields,
	message_id: Identifier,
	sent_at: IsoDateTime,
	thread_id: Identifier,
});

const WorkspaceFileRollbackInput = Schema.Struct({
	...WorkspaceChangeRollbackRequest.fields,
	message_id: Identifier,
	sent_at: IsoDateTime,
	thread_id: Identifier,
});

/** Carries the envelope attribution and replacement request accepted by the controlled workspace service. */
export type WorkspaceFileReplaceInput = typeof WorkspaceFileReplaceInput.Type;

/** Carries the envelope metadata and review request accepted by the controlled workspace service. */
export type WorkspaceFileReviewInput = typeof WorkspaceFileReviewInput.Type;

/** Carries the envelope metadata and rollback request accepted by the controlled workspace service. */
export type WorkspaceFileRollbackInput = typeof WorkspaceFileRollbackInput.Type;

/** Returns either a committed workspace change or a durable pending approval request. */
export type WorkspaceFileReplaceAcceptance =
	| WorkspaceChangeCommit
	| WorkspaceReplaceApprovalAcceptance;

/** Reports a source-free controlled workspace file failure. */
export class WorkspaceFileServiceError extends Data.TaggedError("WorkspaceFileServiceError")<{
	readonly operation: "read" | "replace" | "review" | "rollback";
	readonly reason: "approval_conflict" | "changed" | "diff_limit" | "failed" | "invariant";
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
		) => Effect.Effect<WorkspaceFileReplaceAcceptance, WorkspaceFileServiceError>;
		readonly ExecuteApproved: (
			approval_id: string,
		) => Effect.Effect<void, WorkspaceFileServiceError>;
		readonly SettleDeniedApproval: (
			approval_id: string,
		) => Effect.Effect<void, WorkspaceFileServiceError>;
		readonly Review: (
			input: WorkspaceFileReviewInput,
		) => Effect.Effect<WorkspaceChangeCommit, WorkspaceFileServiceError>;
		readonly Rollback: (
			input: WorkspaceFileRollbackInput,
		) => Effect.Effect<WorkspaceChangeCommit, WorkspaceFileServiceError>;
	}
>()("Artisan/WorkspaceFileService") {}

function failed(operation: WorkspaceFileServiceError["operation"]) {
	return new WorkspaceFileServiceError({ operation, reason: "failed" });
}

function changed(operation: "replace" | "rollback") {
	return new WorkspaceFileServiceError({ operation, reason: "changed" });
}

function approval_conflict() {
	return new WorkspaceFileServiceError({ operation: "replace", reason: "approval_conflict" });
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
		approval_request: input.approval_request,
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

function review_fingerprint(input: WorkspaceFileReviewInput) {
	return JSON.stringify({
		_tag: "review",
		change_id: input.change_id,
		message_id: input.message_id,
		sent_at: input.sent_at,
		thread_id: input.thread_id,
	});
}

function rollback_fingerprint(input: WorkspaceFileRollbackInput) {
	return JSON.stringify({
		_tag: "rollback",
		change_id: input.change_id,
		expected_after: input.expected_after,
		message_id: input.message_id,
		sent_at: input.sent_at,
		thread_id: input.thread_id,
	});
}

type WorkspaceReplaceExecutionMode =
	| { readonly _tag: "request" }
	| {
			readonly _tag: "approved";
			readonly approval_id: string;
			readonly prepared_diff: PreparedWorkspaceChangeDiff;
			readonly request_fingerprint: string;
	  };

/** Builds the controlled workspace file service from its durable and filesystem capabilities. */
export const WorkspaceFileServiceLive = Layer.effect(
	WorkspaceFileService,
	Effect.gen(function* () {
		const authority = yield* WorkspaceMutationAuthority;
		const crypto = yield* Crypto.Crypto;
		const diffs = yield* WorkspaceChangeDiffService;
		const evidence = yield* WorkspaceEvidenceRecorder;
		const payloads = yield* WorkspaceMutationPayloadStore;
		const registry = yield* WorkspaceBoundedRegularFileStoreRegistry;
		const repository = yield* WorkspaceChangeRepository;
		const approvals = yield* WorkspaceReplaceApprovalRepository;
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
		const ComputeMetadataFingerprint = (metadata: string) =>
			Effect.gen(function* () {
				const bytes = new TextEncoder().encode(metadata);
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
			input: Pick<
				WorkspaceFileReplaceInput,
				"change_id" | "expected_before" | "message_id" | "thread_id"
			>,
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

		const RunReplace = (
			input: WorkspaceFileReplaceInput,
			mode: WorkspaceReplaceExecutionMode,
		) =>
			Schema.decodeUnknownEffect(WorkspaceFileReplaceInput, { onExcessProperty: "error" })(
				input,
			).pipe(
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const replacement = yield* EncodeWorkspaceText(decoded.content);
						const intended_after = yield* ComputeIdentity(replacement);
						const request_fingerprint =
							mode._tag === "approved"
								? mode.request_fingerprint
								: yield* ComputeFingerprint(decoded, intended_after);
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
								mode._tag === "request" &&
								error instanceof WorkspaceMutationAuthorityConflict &&
								(error.reason === "operation_conflict" ||
									error.reason === "operation_rejected")
							) {
								const existing_approval = yield* approvals.ReadByMessage(
									decoded.message_id,
								);

								if (Option.isSome(existing_approval)) {
									if (error.reason === "operation_conflict") {
										return yield* Effect.fail(approval_conflict());
									}

									return existing_approval.value;
								}
							}

							if (
								error instanceof WorkspaceMutationAuthorityConflict &&
								error.reason === "operation_rejected"
							) {
								yield* SettleRejected(decoded, intended_after);

								return yield* Effect.fail(changed("replace"));
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
						const Prepare = (payload: {
							readonly expected: Uint8Array;
							readonly replacement: Uint8Array;
						}) =>
							diffs.Prepare({
								after: payload.replacement,
								after_identity: intended_after,
								before: payload.expected,
								before_identity: decoded.expected_before,
								change_id: decoded.change_id,
								message_id: decoded.message_id,
								path: decoded.path,
								thread_id: decoded.thread_id,
								workspace_id: decoded.workspace_id,
							});
						const PrepareBeforeMutation = (payload: {
							readonly expected: Uint8Array;
							readonly replacement: Uint8Array;
						}) =>
							Prepare(payload).pipe(
								Effect.catch((error) =>
									Effect.gen(function* () {
										yield* repository.RejectChanged(decoded.message_id);
										yield* SettleRejected(decoded, intended_after);

										return yield* Effect.fail(error);
									}),
								),
							);
						const FinalizeCommit = (
							payload: {
								readonly expected: Uint8Array;
								readonly replacement: Uint8Array;
							},
							prepared_diff: PreparedWorkspaceChangeDiff,
						) =>
							Effect.gen(function* () {
								yield* admission.store.FinalizeRegularFileReplacement({
									expected: payload.expected,
									maximum_bytes: workspace_text_maximum_bytes,
									operation_id: decoded.message_id,
									path: decoded.path,
									replacement: payload.replacement,
								});
								const commit = yield* repository.CommitRecorded(
									decoded.message_id,
									prepared_diff,
								);

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

								if (mode._tag === "approved") {
									yield* approvals.MarkApplied(mode.approval_id);
								}

								return commit;
							});
						const CompleteDuplicate = (event: WorkspaceChangeCommit["event"]) =>
							Effect.gen(function* () {
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

								if (mode._tag === "approved") {
									yield* approvals.MarkApplied(mode.approval_id);
								}

								return { event, status: "duplicate" as const };
							});
						const RejectObservedChange = () =>
							Effect.gen(function* () {
								yield* SettleRejected(decoded, intended_after);

								return yield* Effect.fail(changed("replace"));
							});
						const RecoverUnavailablePayload = (
							error: WorkspaceMutationPayloadStoreUnavailable,
						) =>
							repository
								.ReconcileChanged({
									message_id: decoded.message_id,
									observation: "preflight_changed",
								})
								.pipe(
									Effect.flatMap((reconciliation) =>
										pipe(
											Match.value(reconciliation),
											Match.tagsExhaustive({
												applied: () => Effect.fail(error),
												committed: ({ event }) =>
													CompleteDuplicate(event).pipe(
														Effect.map((commit) => ({
															_tag: "completed" as const,
															commit,
														})),
													),
												rejected: RejectObservedChange,
												staged: () => Effect.fail(error),
											}),
										),
									),
								);
						const ResolvePreflight = (reconciliation: WorkspaceChangeReconciliation) =>
							pipe(
								Match.value(reconciliation),
								Match.tagsExhaustive({
									applied: () =>
										payloads.Resume(payload_input).pipe(
											Effect.matchEffect({
												onFailure: (error) =>
													error instanceof
													WorkspaceMutationPayloadStoreUnavailable
														? RecoverUnavailablePayload(error)
														: Effect.fail(error),
												onSuccess: (payload) =>
													Prepare(payload)
														.pipe(
															Effect.flatMap((prepared_diff) =>
																FinalizeCommit(
																	payload,
																	prepared_diff,
																),
															),
														)
														.pipe(
															Effect.map((commit) => ({
																_tag: "completed" as const,
																commit,
															})),
														),
											}),
										),
									committed: ({ event }) =>
										CompleteDuplicate(event).pipe(
											Effect.map((commit) => ({
												_tag: "completed" as const,
												commit,
											})),
										),
									rejected: RejectObservedChange,
									staged: () =>
										payloads.Resume(payload_input).pipe(
											Effect.matchEffect({
												onFailure: (error) =>
													error instanceof
													WorkspaceMutationPayloadStoreUnavailable
														? RecoverUnavailablePayload(error)
														: Effect.fail(error),
												onSuccess: (payload) =>
													Effect.succeed({
														_tag: "ready" as const,
														payload,
													}),
											}),
										),
								}),
							);
						const Stage = (expected: Uint8Array) =>
							payloads.Stage({ ...payload_input, expected, replacement });
						const ValidateAndStage = () =>
							Effect.gen(function* () {
								const current = yield* admission.store.ReadRegularFile(
									decoded.path,
									workspace_text_maximum_bytes,
								);
								yield* DecodeWorkspaceText(current);
								const current_identity = yield* ComputeIdentity(current);

								if (!identities_match(current_identity, decoded.expected_before)) {
									const reconciliation = yield* repository.ReconcileChanged({
										message_id: decoded.message_id,
										observation: "preflight_changed",
									});

									return yield* ResolvePreflight(reconciliation);
								}

								yield* Stage(current);

								return {
									_tag: "ready" as const,
									payload: yield* payloads.Resume(payload_input),
								};
							});
						const ResumeOrStage = () =>
							payloads.Resume(payload_input).pipe(
								Effect.matchEffect({
									onFailure: (error) =>
										error instanceof WorkspaceMutationPayloadStoreUnavailable
											? Effect.gen(function* () {
													const has_record =
														yield* payloads.HasRecord(payload_input);

													if (has_record) {
														return yield* Effect.fail(error);
													}

													return yield* ValidateAndStage();
												})
											: Effect.fail(error),
									onSuccess: (payload) =>
										Effect.succeed({ _tag: "ready" as const, payload }),
								}),
							);

						if (admission.claim.operation.action !== "replace") {
							return yield* Effect.fail(failed("replace"));
						}

						if (mode._tag === "request") {
							const existing_approval = yield* approvals.ReadByMessage(
								decoded.message_id,
							);

							if (Option.isSome(existing_approval)) {
								return existing_approval.value;
							}
						}

						if (admission.claim._tag === "duplicate") {
							return yield* CompleteDuplicate(admission.claim.event);
						}

						if (admission.claim.operation.lifecycle === "applied") {
							const payload = yield* payloads.Resume(payload_input);
							const prepared_diff =
								mode._tag === "approved"
									? mode.prepared_diff
									: yield* Prepare(payload);

							return yield* FinalizeCommit(payload, prepared_diff);
						}

						const preparation = yield* admission.claim._tag === "claimed"
							? ValidateAndStage()
							: ResumeOrStage();

						if (preparation._tag === "completed") {
							return preparation.commit;
						}

						const payload = preparation.payload;
						const prepared_diff =
							mode._tag === "approved"
								? mode.prepared_diff
								: yield* PrepareBeforeMutation(payload);

						yield* snapshots.Stage({
							change_id: decoded.change_id,
							content: payload.expected,
							expected_identity: decoded.expected_before,
							thread_id: decoded.thread_id,
						});

						if (mode._tag === "request") {
							const requires_approval =
								admission.authority.approval === "always" ||
								(admission.authority.approval === "on_request" &&
									decoded.approval_request !== undefined);

							if (requires_approval) {
								return yield* approvals.Request({
									operation: admission.claim.operation,
									policy: admission.authority.approval,
									prepared_diff,
									reason:
										decoded.approval_request?.reason ??
										default_workspace_replace_approval_reason,
								});
							}
						} else {
							yield* approvals.MarkExecuting(mode.approval_id);
						}

						const replace = yield* admission.store.ReplaceRegularFile({
							expected: payload.expected,
							maximum_bytes: workspace_text_maximum_bytes,
							operation_id: decoded.message_id,
							path: decoded.path,
							replacement: payload.replacement,
						});

						if (replace._tag === "Changed") {
							const reconciliation = yield* repository.ReconcileChanged({
								message_id: decoded.message_id,
								observation: "native_changed",
							});

							return yield* pipe(
								Match.value(reconciliation),
								Match.tagsExhaustive({
									applied: () => FinalizeCommit(payload, prepared_diff),
									committed: ({ event }) => CompleteDuplicate(event),
									rejected: RejectObservedChange,
									staged: () => Effect.fail(failed("replace")),
								}),
							);
						}

						yield* repository.MarkApplied({
							_tag: "replace",
							message_id: decoded.message_id,
							result_identity: intended_after,
						});

						return yield* FinalizeCommit(payload, prepared_diff);
					}),
				),
				Effect.mapError((error) =>
					error instanceof WorkspaceFileServiceError
						? error
						: error instanceof WorkspaceChangeDiffLimit
							? new WorkspaceFileServiceError({
									operation: "replace",
									reason: "diff_limit",
								})
							: error instanceof WorkspaceReplaceApprovalConflict
								? approval_conflict()
								: error instanceof WorkspaceReplaceApprovalInvariant
									? new WorkspaceFileServiceError({
											operation: "replace",
											reason: "invariant",
										})
									: failed("replace"),
				),
			);

		const Replace = (input: WorkspaceFileReplaceInput) =>
			RunReplace(input, { _tag: "request" });

		const ExecuteApproved = (approval_id: string) =>
			Effect.gen(function* () {
				const execution = yield* approvals.ReadExecution(approval_id);
				const operation_option = yield* repository.ReadOperation(execution.message_id);

				if (
					Option.isNone(operation_option) ||
					operation_option.value.action !== "replace"
				) {
					return yield* Effect.fail(failed("replace"));
				}

				const operation = operation_option.value;
				const payload_input = {
					action: "replace" as const,
					expected_identity: execution.approval.before_identity,
					message_id: execution.message_id,
					replacement_identity: execution.approval.after_identity,
					thread_id: execution.approval.thread_id,
				};
				const ValidateTerminalReadmission = () =>
					authority
						.ClaimReplace({
							_tag: "replace",
							agent_id: execution.approval.agent_id,
							change_id: execution.approval.change_id,
							expected_before: execution.approval.before_identity,
							intended_after: execution.approval.after_identity,
							message_id: execution.message_id,
							path: execution.approval.path,
							...(execution.raw_origin === undefined
								? {}
								: { raw_origin: execution.raw_origin }),
							request_fingerprint: execution.request_fingerprint,
							run_id: execution.approval.run_id,
							sent_at: execution.sent_at,
							thread_id: execution.approval.thread_id,
							workspace_id: execution.approval.workspace_id,
						})
						.pipe(Effect.result);

				if (operation.lifecycle === "committed") {
					const readmission = yield* ValidateTerminalReadmission();

					if (
						Result.isFailure(readmission) ||
						readmission.success.claim._tag !== "duplicate" ||
						readmission.success.claim.operation.lifecycle !== "committed"
					) {
						return yield* Effect.fail(failed("replace"));
					}

					yield* evidence.RecordFilesystemMutation({
						agent_id: execution.approval.agent_id,
						operation: "write",
						operation_id: execution.message_id,
						path: execution.approval.path,
						...(execution.raw_origin === undefined
							? {}
							: { raw_origin: execution.raw_origin }),
						run_id: execution.approval.run_id,
						thread_id: execution.approval.thread_id,
					});
					yield* repository.MarkEvidenceRecorded(execution.message_id);
					yield* payloads.Consume(payload_input);
					yield* approvals.MarkApplied(approval_id);

					return;
				}

				if (operation.lifecycle === "rejected") {
					const readmission = yield* ValidateTerminalReadmission();

					if (
						Result.isSuccess(readmission) ||
						!(readmission.failure instanceof WorkspaceMutationAuthorityConflict) ||
						readmission.failure.reason !== "operation_rejected"
					) {
						return yield* Effect.fail(failed("replace"));
					}

					yield* SettleRejected(
						{
							change_id: execution.approval.change_id,
							expected_before: execution.approval.before_identity,
							message_id: execution.message_id,
							thread_id: execution.approval.thread_id,
						},
						execution.approval.after_identity,
					);
					yield* approvals.MarkRejected(approval_id);

					return;
				}

				const payload = yield* payloads.Resume(payload_input);
				const content = yield* DecodeWorkspaceText(payload.replacement);
				const input: WorkspaceFileReplaceInput = {
					agent_id: execution.approval.agent_id,
					change_id: execution.approval.change_id,
					content,
					expected_before: execution.approval.before_identity,
					message_id: execution.message_id,
					path: execution.approval.path,
					...(execution.raw_origin === undefined
						? {}
						: { raw_origin: execution.raw_origin }),
					run_id: execution.approval.run_id,
					sent_at: execution.sent_at,
					thread_id: execution.approval.thread_id,
					workspace_id: execution.approval.workspace_id,
				};

				const result = yield* RunReplace(input, {
					_tag: "approved",
					approval_id,
					prepared_diff: execution.prepared_diff,
					request_fingerprint: execution.request_fingerprint,
				}).pipe(Effect.result);

				if (Result.isFailure(result)) {
					if (
						result.failure instanceof WorkspaceFileServiceError &&
						result.failure.reason === "changed"
					) {
						yield* approvals.MarkRejected(approval_id);

						return;
					}

					return yield* Effect.fail(result.failure);
				}
			}).pipe(
				Effect.mapError((error) =>
					error instanceof WorkspaceFileServiceError ? error : failed("replace"),
				),
			);

		const SettleDeniedApproval = (approval_id: string) =>
			Effect.gen(function* () {
				const denial = yield* approvals.ReadDenied(approval_id);
				const operation_option = yield* repository.ReadOperation(denial.message_id);

				if (
					Option.isNone(operation_option) ||
					operation_option.value.action !== "replace"
				) {
					return yield* Effect.fail(failed("replace"));
				}

				const operation = operation_option.value;

				if (operation.lifecycle === "claimed") {
					yield* repository.RejectChanged(denial.message_id);
				} else if (operation.lifecycle !== "rejected") {
					return yield* Effect.fail(failed("replace"));
				}

				yield* snapshots.DiscardRejectedReplace({
					change_id: denial.approval.change_id,
					expected_identity: denial.approval.before_identity,
					replace_message_id: denial.message_id,
					thread_id: denial.approval.thread_id,
				});
				yield* payloads.Consume({
					action: "replace",
					expected_identity: denial.approval.before_identity,
					message_id: denial.message_id,
					replacement_identity: denial.approval.after_identity,
					thread_id: denial.approval.thread_id,
				});
			}).pipe(Effect.mapError(() => failed("replace")));

		const Review = (input: WorkspaceFileReviewInput) =>
			Schema.decodeUnknownEffect(WorkspaceFileReviewInput, { onExcessProperty: "error" })(
				input,
			).pipe(
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const request_fingerprint = yield* ComputeMetadataFingerprint(
							review_fingerprint(decoded),
						);
						const claim = yield* repository.ClaimReview({
							_tag: "review",
							change_id: decoded.change_id,
							message_id: decoded.message_id,
							request_fingerprint,
							sent_at: decoded.sent_at,
							thread_id: decoded.thread_id,
						});

						if (claim._tag === "duplicate") {
							return { event: claim.event, status: "duplicate" as const };
						}

						return yield* repository.CommitReviewed(decoded.message_id);
					}),
				),
				Effect.mapError((error) =>
					error instanceof WorkspaceFileServiceError ? error : failed("review"),
				),
			);

		const Rollback = (input: WorkspaceFileRollbackInput) =>
			Schema.decodeUnknownEffect(WorkspaceFileRollbackInput, { onExcessProperty: "error" })(
				input,
			).pipe(
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const request_fingerprint = yield* ComputeMetadataFingerprint(
							rollback_fingerprint(decoded),
						);
						const admission_result = yield* authority
							.ClaimRollback({
								_tag: "rollback",
								change_id: decoded.change_id,
								expected_after: decoded.expected_after,
								message_id: decoded.message_id,
								request_fingerprint,
								sent_at: decoded.sent_at,
								thread_id: decoded.thread_id,
							})
							.pipe(Effect.result);

						if (Result.isFailure(admission_result)) {
							return yield* Effect.fail(admission_result.failure);
						}

						const admission = admission_result.success;
						const source = admission.source;
						const payload_input = {
							action: "rollback" as const,
							expected_identity: source.after_identity,
							message_id: decoded.message_id,
							replacement_identity: source.before_identity,
							thread_id: decoded.thread_id,
						};
						const SettleRejected = () => payloads.Consume(payload_input);
						const RecordEvidence = () =>
							evidence.RecordFilesystemMutation({
								operation: "write",
								operation_id: decoded.message_id,
								path: source.path,
								thread_id: decoded.thread_id,
							});
						const CompleteDuplicate = (event: WorkspaceChangeCommit["event"]) =>
							Effect.gen(function* () {
								yield* snapshots.Consume({
									change_id: decoded.change_id,
									rollback_message_id: decoded.message_id,
									thread_id: decoded.thread_id,
								});
								yield* RecordEvidence();
								yield* repository.MarkEvidenceRecorded(decoded.message_id);
								yield* payloads.Consume(payload_input);

								return { event, status: "duplicate" as const };
							});

						if (admission._tag === "rejected") {
							yield* SettleRejected();

							return yield* Effect.fail(changed("rollback"));
						}

						if (admission._tag === "duplicate") {
							return yield* CompleteDuplicate(admission.claim.event);
						}

						const FinalizeCommit = (payload: {
							readonly expected: Uint8Array;
							readonly replacement: Uint8Array;
						}) =>
							Effect.gen(function* () {
								yield* admission.store.FinalizeRegularFileReplacement({
									expected: payload.expected,
									maximum_bytes: workspace_text_maximum_bytes,
									operation_id: decoded.message_id,
									path: source.path,
									replacement: payload.replacement,
								});
								const commit = yield* repository.CommitRolledBack(
									decoded.message_id,
								);

								yield* snapshots.Consume({
									change_id: decoded.change_id,
									rollback_message_id: decoded.message_id,
									thread_id: decoded.thread_id,
								});
								yield* RecordEvidence();
								yield* repository.MarkEvidenceRecorded(decoded.message_id);
								yield* payloads.Consume(payload_input);

								return commit;
							});
						const RejectObservedChange = () =>
							Effect.gen(function* () {
								yield* SettleRejected();

								return yield* Effect.fail(changed("rollback"));
							});
						const RecoverUnavailablePayload = (
							error: WorkspaceMutationPayloadStoreUnavailable,
						) =>
							repository
								.ReconcileChanged({
									message_id: decoded.message_id,
									observation: "preflight_changed",
								})
								.pipe(
									Effect.flatMap((reconciliation) =>
										pipe(
											Match.value(reconciliation),
											Match.tagsExhaustive({
												applied: () => Effect.fail(error),
												committed: ({ event }) =>
													CompleteDuplicate(event).pipe(
														Effect.map((commit) => ({
															_tag: "completed" as const,
															commit,
														})),
													),
												rejected: RejectObservedChange,
												staged: () => Effect.fail(error),
											}),
										),
									),
								);
						const ResolvePreflight = (reconciliation: WorkspaceChangeReconciliation) =>
							pipe(
								Match.value(reconciliation),
								Match.tagsExhaustive({
									applied: () =>
										payloads.Resume(payload_input).pipe(
											Effect.matchEffect({
												onFailure: (error) =>
													error instanceof
													WorkspaceMutationPayloadStoreUnavailable
														? RecoverUnavailablePayload(error)
														: Effect.fail(error),
												onSuccess: (payload) =>
													FinalizeCommit(payload).pipe(
														Effect.map((commit) => ({
															_tag: "completed" as const,
															commit,
														})),
													),
											}),
										),
									committed: ({ event }) =>
										CompleteDuplicate(event).pipe(
											Effect.map((commit) => ({
												_tag: "completed" as const,
												commit,
											})),
										),
									rejected: RejectObservedChange,
									staged: () =>
										payloads.Resume(payload_input).pipe(
											Effect.matchEffect({
												onFailure: (error) =>
													error instanceof
													WorkspaceMutationPayloadStoreUnavailable
														? RecoverUnavailablePayload(error)
														: Effect.fail(error),
												onSuccess: (payload) =>
													Effect.succeed({
														_tag: "ready" as const,
														payload,
													}),
											}),
										),
								}),
							);

						const ReadAndStage = () =>
							Effect.gen(function* () {
								const original = yield* snapshots.Read({
									change_id: decoded.change_id,
									expected_identity: source.before_identity,
									thread_id: decoded.thread_id,
								});
								const current = yield* admission.store.ReadRegularFile(
									source.path,
									workspace_text_maximum_bytes,
								);
								yield* DecodeWorkspaceText(original);
								yield* DecodeWorkspaceText(current);
								const current_identity = yield* ComputeIdentity(current);
								const original_identity = yield* ComputeIdentity(original);

								if (
									!identities_match(current_identity, source.after_identity) ||
									!identities_match(original_identity, source.before_identity)
								) {
									const reconciliation = yield* repository.ReconcileChanged({
										message_id: decoded.message_id,
										observation: "preflight_changed",
									});

									return yield* ResolvePreflight(reconciliation);
								}

								yield* payloads.Stage({
									...payload_input,
									expected: current,
									replacement: original,
								});

								return {
									_tag: "ready" as const,
									payload: yield* payloads.Resume(payload_input),
								};
							});
						const ResumeOrStage = () =>
							payloads.Resume(payload_input).pipe(
								Effect.matchEffect({
									onFailure: (error) =>
										error instanceof WorkspaceMutationPayloadStoreUnavailable
											? Effect.gen(function* () {
													const has_record =
														yield* payloads.HasRecord(payload_input);

													if (has_record) {
														return yield* Effect.fail(error);
													}

													return yield* ReadAndStage();
												})
											: Effect.fail(error),
									onSuccess: (payload) =>
										Effect.succeed({ _tag: "ready" as const, payload }),
								}),
							);

						if (admission.claim.operation.lifecycle === "applied") {
							const payload = yield* payloads.Resume(payload_input);

							return yield* FinalizeCommit(payload);
						}

						const preparation = yield* admission.claim._tag === "claimed"
							? ReadAndStage()
							: ResumeOrStage();

						if (preparation._tag === "completed") {
							return preparation.commit;
						}

						const payload = preparation.payload;

						const replace = yield* admission.store.ReplaceRegularFile({
							expected: payload.expected,
							maximum_bytes: workspace_text_maximum_bytes,
							operation_id: decoded.message_id,
							path: source.path,
							replacement: payload.replacement,
						});

						if (replace._tag === "Changed") {
							const reconciliation = yield* repository.ReconcileChanged({
								message_id: decoded.message_id,
								observation: "native_changed",
							});

							return yield* pipe(
								Match.value(reconciliation),
								Match.tagsExhaustive({
									applied: () => FinalizeCommit(payload),
									committed: ({ event }) => CompleteDuplicate(event),
									rejected: RejectObservedChange,
									staged: () => Effect.fail(failed("rollback")),
								}),
							);
						}

						yield* repository.MarkApplied({
							_tag: "rollback",
							message_id: decoded.message_id,
						});

						return yield* FinalizeCommit(payload);
					}),
				),
				Effect.mapError((error) =>
					error instanceof WorkspaceFileServiceError ? error : failed("rollback"),
				),
			);

		return { ExecuteApproved, Read, Replace, Review, Rollback, SettleDeniedApproval };
	}),
);
