import {
	Context,
	Data,
	Effect,
	Fiber,
	Layer,
	Option,
	Ref,
	Schedule,
	Scope,
	Stream,
	SubscriptionRef,
} from "effect";

import type {
	ArtisanApprovalListQueryResult,
	ArtisanToolInvocationListQueryResult,
	ArtisanToolRegistryListQueryResult,
	CapabilityRegistrySnapshot,
	CapabilityDetail,
	CapabilityOAuthTokenStatus,
	GlobalGuidanceSnapshot,
	GitDiffQueryResult,
	GitWorkspaceQueryResult,
	ModelBehaviourSnapshot,
	OrchestrationGraph,
	OrchestrationGroupListSnapshot,
	PreviewTarget,
	PreviewInspectionResult,
	PreviewInspectionSession,
	RichLinkAssetMetadata,
	RoutineDetail,
	RoutineRegistrySnapshot,
	SurfaceSnapshot,
	SurfaceUsageAggregateSnapshot,
	TerminalSession,
	ThreadListItem,
	ThreadSessionSnapshot,
	ThreadTranscriptSnapshot,
	ThreadWorkItem,
	WorkspaceChangeDiffQueryResult,
	WorkspaceChangeListQueryResult,
	WorkspaceConflictListQueryResult,
	WorkspaceFileDiscoveryQueryResult,
	WorkspaceFileReadQueryResult,
} from "@artisan/protocol";
import {
	ArtisanClient,
	type ArtisanApprovalListInput,
	type ArtisanApprovalResolveInput,
	type ArtisanCommandInput,
	type ArtisanCapabilityDetailInput,
	type ArtisanCapabilityOAuthInput,
	type ArtisanGitDiffInput,
	type ArtisanGitIndexMutationInput,
	type ArtisanGitMutationResolveInput,
	type ArtisanGitWorkspaceInput,
	type ArtisanMarketplaceBrowseInput,
	type ArtisanPreviewAssetMetadataInput,
	type ArtisanPreviewInspectionInput,
	type ArtisanPreviewInspectionOpenInput,
	type ArtisanPreviewTargetInput,
	type ArtisanRoutineDetailInput,
	type ArtisanToolExecuteInput,
	type ArtisanToolInvocationListInput,
	type ArtisanToolRegistryListInput,
	type ArtisanWorkspaceChangeDiffInput,
	type ArtisanWorkspaceChangeListInput,
	type ArtisanWorkspaceChangeReviewInput,
	type ArtisanWorkspaceChangeRollbackInput,
	type ArtisanWorkspaceFileDiscoveryInput,
	type ArtisanWorkspaceFileReadInput,
	type ArtisanWorkspaceFileReplaceInput,
	type ThreadListUpdate,
	type ThreadTranscriptUpdate,
} from "@artisan/transport/client";

import {
	FrontendConnectionLifecycle,
	type FrontendConnectionPhase,
	type FrontendConnectionState,
} from "../runtime/desktop-message-port-connector";

export type LiveWorkspacePhase =
	| "connecting"
	| "ready"
	| "reconnecting"
	| "stale"
	| "error"
	| "empty";

/** Renderer action result used to reconcile Monaco after a conditional backend replacement. */
export type LiveWorkspaceFileReplaceOutcome =
	| { readonly _tag: "Saved"; readonly file: WorkspaceFileReadQueryResult }
	| {
			readonly _tag: "Conflict";
			readonly file: WorkspaceFileReadQueryResult;
			readonly message: string;
	  }
	| { readonly _tag: "Failed"; readonly message: string };

/** Complete renderer-safe control plane. Actions retain the transport's input/result types. */
export type LiveWorkspaceActions = Pick<
	typeof ArtisanClient.Service,
	| "Command"
	| "OpenTerminalOutput"
	| "OpenAsset"
	| "ResolveRichLink"
	| "RegisterPreviewTarget"
	| "ProbePreviewTarget"
	| "RemovePreviewTarget"
	| "SetPreviewTargetState"
	| "LaunchPreviewInExternalBrowser"
	| "ClosePreviewInspectionSession"
	| "PreviewRoutineInstall"
	| "RequestRoutineInstall"
	| "DecideRoutineInstall"
	| "EnableRoutine"
	| "DisableRoutine"
	| "RemoveRoutine"
	| "RollbackRoutine"
	| "SyncRoutine"
	| "ResolveRoutineDrift"
	| "RequestRoutineDriftOverwrite"
	| "DecideRoutineDriftOverwrite"
	| "InvokeRoutine"
	| "DiscoverNpxSkills"
	| "ImportNpxSkills"
	| "PreviewCapabilityConnect"
	| "GetCapabilityOAuthStatus"
	| "RequestCapabilityConnect"
	| "DecideCapabilityConnect"
	| "StartCapability"
	| "ReconnectCapability"
	| "CheckCapabilityHealth"
	| "DisconnectCapability"
	| "RestartCapability"
	| "UninstallCapability"
	| "EnableCapability"
	| "DisableCapability"
	| "RemoveCapability"
	| "SyncCapability"
	| "ResolveCapabilityDrift"
	| "RequestCapabilityDriftOverwrite"
	| "DecideCapabilityDriftOverwrite"
	| "RequestCapabilityInvocation"
	| "DecideCapabilityInvocation"
	| "InvokeCapability"
	| "BeginCapabilityOAuth"
	| "CompleteCapabilityOAuth"
	| "RefreshCapabilityOAuth"
	| "RevokeCapabilityOAuth"
	| "UpdateGlobalGuidance"
	| "SelectGlobalGuidance"
	| "ResolveGlobalGuidanceDrift"
	| "RetryGlobalGuidanceSync"
	| "UpdateModelBehaviour"
	| "ResolveModelBehaviourDrift"
	| "RetryModelBehaviourSync"
	| "UpdateThreadSessionPolicy"
	| "GetThreadRetentionPolicy"
	| "UpdateThreadRetentionPolicy"
>;

export interface LiveWorkspaceSnapshot {
	readonly error: Option.Option<string>;
	/** Errors are isolated by projection so one failed query never erases usable state. */
	readonly errors: Readonly<Record<string, string>>;
	readonly global_guidance: Option.Option<GlobalGuidanceSnapshot>;
	readonly model_behaviour: Option.Option<ModelBehaviourSnapshot>;
	readonly phase: LiveWorkspacePhase;
	readonly selected_thread_id: Option.Option<string>;
	readonly selected_group_id: Option.Option<string>;
	readonly orchestration_graph: Option.Option<OrchestrationGraph>;
	readonly orchestration_groups: Option.Option<OrchestrationGroupListSnapshot>;
	readonly transcript: Option.Option<ThreadTranscriptSnapshot>;
	readonly session: Option.Option<ThreadSessionSnapshot>;
	readonly surface_items: Option.Option<SurfaceSnapshot>;
	readonly surface_usage: Option.Option<SurfaceUsageAggregateSnapshot>;
	readonly workspace_conflicts: Option.Option<WorkspaceConflictListQueryResult>;
	readonly workspace_changes: Option.Option<WorkspaceChangeListQueryResult>;
	readonly workspace_file_page: Option.Option<WorkspaceFileDiscoveryQueryResult>;
	readonly workspace_file: Option.Option<WorkspaceFileReadQueryResult>;
	readonly workspace_change_diff: Option.Option<WorkspaceChangeDiffQueryResult>;
	readonly git_workspace: Option.Option<GitWorkspaceQueryResult>;
	readonly git_diff: Option.Option<GitDiffQueryResult>;
	readonly terminals: ReadonlyArray<TerminalSession>;
	readonly terminal_output: Readonly<Record<string, string>>;
	readonly preview_targets: ReadonlyArray<PreviewTarget>;
	readonly tool_registry: Option.Option<ArtisanToolRegistryListQueryResult>;
	readonly tool_invocations: Option.Option<ArtisanToolInvocationListQueryResult>;
	readonly tool_approvals: Option.Option<ArtisanApprovalListQueryResult>;
	readonly routines: Option.Option<RoutineRegistrySnapshot>;
	readonly capabilities: Option.Option<CapabilityRegistrySnapshot>;
	readonly routine_detail: Option.Option<RoutineDetail>;
	readonly capability_detail: Option.Option<CapabilityDetail>;
	readonly capability_oauth: Option.Option<CapabilityOAuthTokenStatus>;
	readonly preview_target: Option.Option<PreviewTarget>;
	readonly preview_asset_metadata: Option.Option<RichLinkAssetMetadata>;
	readonly preview_inspection_session: Option.Option<PreviewInspectionSession>;
	readonly preview_inspection_result: Option.Option<PreviewInspectionResult>;
	readonly thread_work: Option.Option<ThreadWorkItem>;
	readonly threads: ReadonlyArray<ThreadListItem>;
}

interface LiveWorkspaceState {
	readonly list_generation: number;
	readonly refresh_generation: number;
	readonly selection_generation: number;
	readonly subscription_generation: number;
	readonly snapshot: LiveWorkspaceSnapshot;
}

const EmptySnapshot: LiveWorkspaceSnapshot = {
	error: Option.none(),
	errors: {},
	global_guidance: Option.none(),
	model_behaviour: Option.none(),
	phase: "connecting",
	selected_thread_id: Option.none(),
	selected_group_id: Option.none(),
	orchestration_graph: Option.none(),
	orchestration_groups: Option.none(),
	transcript: Option.none(),
	session: Option.none(),
	surface_items: Option.none(),
	surface_usage: Option.none(),
	workspace_conflicts: Option.none(),
	workspace_changes: Option.none(),
	workspace_file_page: Option.none(),
	workspace_file: Option.none(),
	workspace_change_diff: Option.none(),
	git_workspace: Option.none(),
	git_diff: Option.none(),
	terminals: [],
	terminal_output: {},
	preview_targets: [],
	tool_registry: Option.none(),
	tool_invocations: Option.none(),
	tool_approvals: Option.none(),
	routines: Option.none(),
	capabilities: Option.none(),
	routine_detail: Option.none(),
	capability_detail: Option.none(),
	capability_oauth: Option.none(),
	preview_target: Option.none(),
	preview_asset_metadata: Option.none(),
	preview_inspection_session: Option.none(),
	preview_inspection_result: Option.none(),
	thread_work: Option.none(),
	threads: [],
};

const EmptyState: LiveWorkspaceState = {
	list_generation: 0,
	refresh_generation: 0,
	selection_generation: 0,
	subscription_generation: 0,
	snapshot: EmptySnapshot,
};

class ThreadListSubscriptionLost extends Data.TaggedError("ThreadListSubscriptionLost")<{
	readonly message: string;
}> {}

const ThreadListSubscriptionRetrySchedule = Schedule.exponential("100 millis").pipe(
	Schedule.upTo({ duration: "1 second", times: 3 }),
);

/** Runs a public projection stream with the same bounded recovery policy as thread-list. */
const RunProjectionSubscription = <
	Update,
	SubscribeError extends { readonly message: string },
	StreamError extends { readonly message: string },
>(
	subscribe: Effect.Effect<Stream.Stream<Update, StreamError>, SubscribeError, Scope.Scope>,
	on_update: (update: Update) => Effect.Effect<void>,
	on_failure: (message: string) => Effect.Effect<void>,
) =>
	subscribe.pipe(
		Effect.flatMap((updates) =>
			Stream.runForEach(updates, on_update).pipe(
				Effect.flatMap(() =>
					Effect.fail(
						new ThreadListSubscriptionLost({
							message: "Projection subscription ended unexpectedly.",
						}),
					),
				),
			),
		),
		Effect.retry({ schedule: ThreadListSubscriptionRetrySchedule }),
		Effect.catch((error) => on_failure(error.message)),
		Effect.asVoid,
	);

export const ToLiveWorkspacePhase = (phase: FrontendConnectionPhase): LiveWorkspacePhase => {
	if (phase === "ready") return "ready";
	if (phase === "reconnecting") return "reconnecting";
	if (phase === "stale") return "stale";
	if (phase === "error" || phase === "unavailable") return "error";
	return "connecting";
};

/** A new ready generation is the only lifecycle state that may reload projections. */
export const ShouldRefreshForConnection = (phase: FrontendConnectionPhase) => phase === "ready";

/** Keeps renderer terminal scrollback bounded while preserving the newest decoded text. */
export const AppendBoundedTerminalOutput = (current: string, appended: string, limit = 32_768) =>
	`${current}${appended}`.slice(-Math.max(1, Math.trunc(limit)));

const selected_thread_exists = (
	selected_thread_id: Option.Option<string>,
	threads: ReadonlyArray<ThreadListItem>,
) =>
	Option.isNone(selected_thread_id) ||
	threads.some((thread) => thread.thread_id === selected_thread_id.value);

const reconcile_selection = (
	snapshot: LiveWorkspaceSnapshot,
	threads: ReadonlyArray<ThreadListItem>,
): LiveWorkspaceSnapshot =>
	selected_thread_exists(snapshot.selected_thread_id, threads)
		? { ...snapshot, threads }
		: {
				...snapshot,
				selected_thread_id: Option.none(),
				selected_group_id: Option.none(),
				orchestration_graph: Option.none(),
				orchestration_groups: Option.none(),
				transcript: Option.none(),
				session: Option.none(),
				surface_items: Option.none(),
				surface_usage: Option.none(),
				workspace_conflicts: Option.none(),
				workspace_changes: Option.none(),
				workspace_file_page: Option.none(),
				workspace_file: Option.none(),
				workspace_change_diff: Option.none(),
				git_workspace: Option.none(),
				git_diff: Option.none(),
				terminals: [],
				terminal_output: {},
				preview_targets: [],
				tool_invocations: Option.none(),
				tool_approvals: Option.none(),
				thread_work: Option.none(),
				threads,
			};

export const ApplyThreadListUpdate = (
	snapshot: LiveWorkspaceSnapshot,
	update:
		| { readonly type: "snapshot"; readonly threads: ReadonlyArray<ThreadListItem> }
		| { readonly type: "upsert"; readonly thread: ThreadListItem }
		| { readonly type: "remove"; readonly thread_id: string },
): LiveWorkspaceSnapshot => {
	/** A list stream snapshot is authoritative, including the legitimate no-thread state. */
	if (update.type === "snapshot")
		return ApplyAuthoritativeThreadRefresh(snapshot, update.threads);
	if (update.type === "remove") {
		return ApplyAuthoritativeThreadRefresh(
			snapshot,
			snapshot.threads.filter((thread) => thread.thread_id !== update.thread_id),
		);
	}

	const threads = snapshot.threads.filter(
		(thread) => thread.thread_id !== update.thread.thread_id,
	);
	return ApplyAuthoritativeThreadRefresh(snapshot, [update.thread, ...threads]);
};

/** Applies a complete backend list as the current authoritative renderer projection. */
export const ApplyAuthoritativeThreadRefresh = (
	snapshot: LiveWorkspaceSnapshot,
	threads: ReadonlyArray<ThreadListItem>,
): LiveWorkspaceSnapshot => {
	const reconciled = reconcile_selection(snapshot, threads);
	const selected_thread_id = Option.isSome(reconciled.selected_thread_id)
		? reconciled.selected_thread_id
		: threads[0] === undefined
			? Option.none<string>()
			: Option.some(threads[0].thread_id);
	const selection_changed =
		Option.getOrUndefined(reconciled.selected_thread_id) !==
		Option.getOrUndefined(selected_thread_id);

	return {
		...reconciled,
		error: Option.none(),
		phase: threads.length === 0 ? "empty" : "ready",
		selected_thread_id,
		thread_work: selection_changed ? Option.none() : reconciled.thread_work,
	};
};

/** Makes subscription loss actionable while retaining the last known backend projection. */
export const ApplyThreadListSubscriptionFailure = (
	snapshot: LiveWorkspaceSnapshot,
	error: string,
): LiveWorkspaceSnapshot => ({
	...snapshot,
	error: Option.some(error),
	phase: "error",
});

/** Retries a dropped authoritative stream with a bounded backoff before reporting its final loss. */
export const RunThreadListSubscription = <
	SubscribeError extends { readonly message: string },
	StreamError extends { readonly message: string },
>(
	subscribe: Effect.Effect<
		Stream.Stream<ThreadListUpdate, StreamError>,
		SubscribeError,
		Scope.Scope
	>,
	on_update: (update: ThreadListUpdate) => Effect.Effect<void>,
	on_failure: (message: string) => Effect.Effect<void>,
) =>
	subscribe.pipe(
		Effect.flatMap((updates) =>
			Stream.runForEach(updates, on_update).pipe(
				Effect.flatMap(() =>
					Effect.fail(
						new ThreadListSubscriptionLost({
							message: "Thread-list subscription ended unexpectedly.",
						}),
					),
				),
			),
		),
		Effect.retry({ schedule: ThreadListSubscriptionRetrySchedule }),
		Effect.catch((error) => on_failure(error.message)),
		Effect.asVoid,
	);

const SelectThreadSnapshot = (snapshot: LiveWorkspaceSnapshot, thread_id: string) => ({
	...snapshot,
	error: Option.none(),
	selected_thread_id: Option.some(thread_id),
	thread_work: Option.none(),
});

/** Rejects a late work query when the renderer has selected another thread. */
export const IsCurrentThreadSelection = (
	snapshot: LiveWorkspaceSnapshot,
	selection_generation: number,
	expected_thread_id: string,
	expected_selection_generation: number,
) =>
	selection_generation === expected_selection_generation &&
	Option.getOrUndefined(snapshot.selected_thread_id) === expected_thread_id;

/** Terminal bytes may update only while the terminal remains in the selected workspace. */
export const IsCurrentTerminalOutputWatcher = (
	snapshot: LiveWorkspaceSnapshot,
	selection_generation: number,
	expected: {
		readonly selection_generation: number;
		readonly terminal_id: string;
		readonly thread_id: string;
		readonly workspace_id: string;
	},
) =>
	IsCurrentThreadSelection(
		snapshot,
		selection_generation,
		expected.thread_id,
		expected.selection_generation,
	) &&
	snapshot.terminals.some(
		(terminal) =>
			terminal.terminal_id === expected.terminal_id &&
			terminal.thread_id === expected.thread_id &&
			terminal.workspace_id === expected.workspace_id,
	);

/** Builds the only renderer-originated message commands permitted by the Codex-only prototype. */
export const BuildLiveWorkspaceMessageCommand = (
	snapshot: LiveWorkspaceSnapshot,
	text: string,
): Option.Option<ArtisanCommandInput> => {
	const trimmed = text.trim();
	const thread_id = Option.getOrUndefined(snapshot.selected_thread_id);
	if (trimmed.length === 0 || thread_id === undefined) return Option.none();

	const session = Option.getOrUndefined(snapshot.session);
	const pending_question = session?.pending_question;
	if (pending_question?.state === "pending")
		return Option.some({
			payload: {
				answers: { answer: [trimmed] },
				question_id: pending_question.question_id,
				type: "intake.respond_question",
			},
			thread_id,
		});

	const thread = snapshot.threads.find((candidate) => candidate.thread_id === thread_id);
	const project = thread?.primary_project;
	if (project === undefined) return Option.none();

	const thread_work = Option.getOrUndefined(snapshot.thread_work);
	const engine_id = thread_work?.engine_id ?? session?.policy.engine_id ?? "codex";
	if (engine_id !== "codex") return Option.none();

	return Option.some({
		...(thread_work === undefined
			? {}
			: { agent_id: thread_work.agent_id, run_id: thread_work.run_id }),
		payload: {
			engine_id,
			mentioned_projects: [project],
			text: trimmed,
			type: "thread.send_message",
			working_directory: project.root_path,
		},
		thread_id,
	});
};

/** Applies only ordered transcript updates; repeated replay entries are ignored by id. */
export const ApplyThreadTranscriptUpdate = (
	snapshot: LiveWorkspaceSnapshot,
	update: ThreadTranscriptUpdate,
): LiveWorkspaceSnapshot => {
	if (update.type === "snapshot")
		return { ...snapshot, transcript: Option.some(update.transcript) };
	if (Option.isNone(snapshot.transcript)) return snapshot;
	const seen = new Set(snapshot.transcript.value.entries.map((entry) => entry.event_id));
	const entries = [
		...snapshot.transcript.value.entries,
		...update.entries.filter((entry) => !seen.has(entry.event_id)),
	];
	return { ...snapshot, transcript: Option.some({ ...snapshot.transcript.value, entries }) };
};

/** Owns renderer-only live projection state; durable records remain backend projections. */
export class LiveWorkspaceStore extends Context.Service<
	LiveWorkspaceStore,
	{
		readonly Changes: Stream.Stream<LiveWorkspaceSnapshot>;
		readonly Actions: LiveWorkspaceActions;
		readonly CreateThread: (title: string) => Effect.Effect<void>;
		readonly Refresh: Effect.Effect<void>;
		readonly SendMessage: (text: string) => Effect.Effect<void>;
		readonly RefreshWorkspaceFiles: (
			input: ArtisanWorkspaceFileDiscoveryInput,
		) => Effect.Effect<void>;
		readonly ReadWorkspaceFile: (
			input: ArtisanWorkspaceFileReadInput,
		) => Effect.Effect<Option.Option<WorkspaceFileReadQueryResult>>;
		readonly ReplaceWorkspaceFile: (
			input: ArtisanWorkspaceFileReplaceInput,
		) => Effect.Effect<LiveWorkspaceFileReplaceOutcome>;
		readonly RefreshWorkspaceChanges: (
			input: ArtisanWorkspaceChangeListInput,
		) => Effect.Effect<void>;
		readonly LoadWorkspaceChangeDiff: (
			input: ArtisanWorkspaceChangeDiffInput,
		) => Effect.Effect<void>;
		readonly ReviewWorkspaceChange: (
			input: ArtisanWorkspaceChangeReviewInput,
		) => Effect.Effect<void>;
		readonly RollbackWorkspaceChange: (
			input: ArtisanWorkspaceChangeRollbackInput,
		) => Effect.Effect<void>;
		readonly RefreshGitWorkspace: (input: ArtisanGitWorkspaceInput) => Effect.Effect<void>;
		readonly RefreshGitDiff: (input: ArtisanGitDiffInput) => Effect.Effect<void>;
		readonly RequestGitIndexMutation: (
			input: ArtisanGitIndexMutationInput,
		) => Effect.Effect<void>;
		readonly ResolveGitMutation: (input: ArtisanGitMutationResolveInput) => Effect.Effect<void>;
		readonly RefreshTerminals: (thread_id: string, workspace_id: string) => Effect.Effect<void>;
		readonly WatchTerminalOutput: (terminal_id: string) => Effect.Effect<void>;
		readonly RefreshPreviewTargets: () => Effect.Effect<void>;
		readonly RefreshTools: (input: ArtisanToolRegistryListInput) => Effect.Effect<void>;
		readonly RefreshToolInvocations: (
			input: ArtisanToolInvocationListInput,
		) => Effect.Effect<void>;
		readonly RefreshToolApprovals: (input: ArtisanApprovalListInput) => Effect.Effect<void>;
		readonly ExecuteTool: (input: ArtisanToolExecuteInput) => Effect.Effect<void>;
		readonly ResolveToolApproval: (input: ArtisanApprovalResolveInput) => Effect.Effect<void>;
		readonly RefreshMarketplace: (input: ArtisanMarketplaceBrowseInput) => Effect.Effect<void>;
		readonly GetRoutineDetail: (input: ArtisanRoutineDetailInput) => Effect.Effect<void>;
		readonly GetCapabilityDetail: (input: ArtisanCapabilityDetailInput) => Effect.Effect<void>;
		readonly GetCapabilityOAuthStatus: (
			input: ArtisanCapabilityOAuthInput,
		) => Effect.Effect<void>;
		readonly GetPreviewTarget: (input: ArtisanPreviewTargetInput) => Effect.Effect<void>;
		readonly GetPreviewAssetMetadata: (
			input: ArtisanPreviewAssetMetadataInput,
		) => Effect.Effect<void>;
		readonly OpenPreviewInspection: (
			input: ArtisanPreviewInspectionOpenInput,
		) => Effect.Effect<void>;
		readonly InspectPreview: (input: ArtisanPreviewInspectionInput) => Effect.Effect<void>;
		readonly SelectOrchestrationGroup: (group_id: string) => Effect.Effect<void>;
		readonly SelectThread: (thread_id: string) => Effect.Effect<void>;
		readonly Snapshot: Effect.Effect<LiveWorkspaceSnapshot>;
	}
>()("Artisan/LiveWorkspaceStore") {}

export const LiveWorkspaceStoreLive = Layer.effect(
	LiveWorkspaceStore,
	Effect.gen(function* () {
		const client = yield* ArtisanClient;
		const lifecycle = yield* FrontendConnectionLifecycle;
		const scope = yield* Scope.Scope;
		const state = yield* SubscriptionRef.make(EmptyState);
		const subscription_fiber = yield* Ref.make<Option.Option<Fiber.Fiber<void, never>>>(
			Option.none(),
		);
		const selected_subscription_fibers = yield* Ref.make<
			ReadonlyArray<Fiber.Fiber<void, never>>
		>([]);
		const group_subscription_fiber = yield* Ref.make<Option.Option<Fiber.Fiber<void, never>>>(
			Option.none(),
		);
		type TerminalOutputWatcher = {
			readonly selection_generation: number;
			readonly terminal_id: string;
			readonly thread_id: string;
			readonly workspace_id: string;
			readonly fiber: Option.Option<Fiber.Fiber<void, never>>;
		};
		const terminal_output_watchers = yield* Ref.make<
			ReadonlyMap<string, TerminalOutputWatcher>
		>(new Map());

		const Update = (update: (current: LiveWorkspaceState) => LiveWorkspaceState) =>
			SubscriptionRef.update(state, update);

		const UpdateAndGet = (update: (current: LiveWorkspaceState) => LiveWorkspaceState) =>
			SubscriptionRef.modify(state, (current) => {
				const next = update(current);
				return [next, next] as const;
			});

		const StopTerminalOutputWatchers = (
			predicate: (watcher: TerminalOutputWatcher) => boolean = () => true,
		) =>
			Effect.gen(function* () {
				const stopped = yield* Ref.modify(terminal_output_watchers, (watchers) => {
					const retained = new Map<string, TerminalOutputWatcher>();
					const interrupted: Array<TerminalOutputWatcher> = [];
					for (const watcher of watchers.values()) {
						if (predicate(watcher)) interrupted.push(watcher);
						else retained.set(watcher.terminal_id, watcher);
					}
					return [interrupted, retained] as const;
				});
				yield* Effect.forEach(
					stopped,
					(watcher) =>
						Option.match(watcher.fiber, {
							onNone: () => Effect.void,
							onSome: Fiber.interrupt,
						}),
					{ discard: true },
				);
			});

		const ClearTerminalOutputWatcher = (watcher: TerminalOutputWatcher) =>
			Ref.update(terminal_output_watchers, (watchers) => {
				const current = watchers.get(watcher.terminal_id);
				if (
					current === undefined ||
					current.selection_generation !== watcher.selection_generation ||
					current.thread_id !== watcher.thread_id ||
					current.workspace_id !== watcher.workspace_id
				)
					return watchers;
				const next = new Map(watchers);
				next.delete(watcher.terminal_id);
				return next;
			});

		const Project = <Value>(
			surface: string,
			load: Effect.Effect<Value, { readonly message: string }>,
			apply: (snapshot: LiveWorkspaceSnapshot, value: Value) => LiveWorkspaceSnapshot,
		) =>
			load.pipe(
				Effect.matchEffect({
					onFailure: (error) =>
						Update((current) => ({
							...current,
							snapshot: {
								...current.snapshot,
								errors: { ...current.snapshot.errors, [surface]: error.message },
							},
						})),
					onSuccess: (value) =>
						Update((current) => {
							const { [surface]: _cleared, ...errors } = current.snapshot.errors;
							return {
								...current,
								snapshot: { ...apply(current.snapshot, value), errors },
							};
						}),
				}),
			);

		const Mutate = <Value>(
			surface: string,
			mutation: Effect.Effect<Value, { readonly message: string }>,
			after: Effect.Effect<void> = Effect.void,
		) =>
			mutation.pipe(
				Effect.flatMap(() => after),
				Effect.matchEffect({
					onFailure: (error) =>
						Update((current) => ({
							...current,
							snapshot: {
								...current.snapshot,
								errors: { ...current.snapshot.errors, [surface]: error.message },
							},
						})),
					onSuccess: () =>
						Update((current) => {
							const { [surface]: _cleared, ...errors } = current.snapshot.errors;
							return { ...current, snapshot: { ...current.snapshot, errors } };
						}),
				}),
			);

		const LoadSelectedThread = (thread_id: string, selection_generation: number) =>
			client.GetThreadWork(thread_id).pipe(
				Effect.matchEffect({
					onFailure: (error) =>
						Update((current) =>
							IsCurrentThreadSelection(
								current.snapshot,
								current.selection_generation,
								thread_id,
								selection_generation,
							)
								? {
										...current,
										snapshot: {
											...current.snapshot,
											error: Option.some(error.message),
										},
									}
								: current,
						),
					onSuccess: (thread_work) =>
						Update((current) =>
							IsCurrentThreadSelection(
								current.snapshot,
								current.selection_generation,
								thread_id,
								selection_generation,
							)
								? {
										...current,
										snapshot: { ...current.snapshot, thread_work },
									}
								: current,
						),
				}),
			);

		/** Projection reads are authoritative; raw event streams never reconstruct history. */
		const LoadThreadSurfaces = (thread_id: string, selection_generation: number) =>
			Effect.all([
				client.GetThreadTranscript({ thread_id }),
				client.ListOrchestrationGroups(thread_id, true),
			]).pipe(
				Effect.matchEffect({
					onFailure: (error) =>
						Update((current) =>
							IsCurrentThreadSelection(
								current.snapshot,
								current.selection_generation,
								thread_id,
								selection_generation,
							)
								? {
										...current,
										snapshot: {
											...current.snapshot,
											error: Option.some(error.message),
										},
									}
								: current,
						),
					onSuccess: ([transcript, orchestration_groups]) =>
						Update((current) => {
							if (
								!IsCurrentThreadSelection(
									current.snapshot,
									current.selection_generation,
									thread_id,
									selection_generation,
								)
							)
								return current;
							const selected_group_id = Option.getOrUndefined(
								current.snapshot.selected_group_id,
							);
							const group_id = orchestration_groups.groups.some(
								(group) => group.group_id === selected_group_id,
							)
								? current.snapshot.selected_group_id
								: orchestration_groups.groups[0] === undefined
									? Option.none<string>()
									: Option.some(orchestration_groups.groups[0].group_id);
							return {
								...current,
								snapshot: {
									...current.snapshot,
									transcript: Option.some(transcript),
									orchestration_groups: Option.some(orchestration_groups),
									selected_group_id: group_id,
									orchestration_graph: Option.none(),
								},
							};
						}),
				}),
			);

		const LoadGraph = (group_id: string, selection_generation: number) =>
			client.GetOrchestrationGraph(group_id).pipe(
				Effect.matchEffect({
					onFailure: (error) =>
						Update((current) =>
							current.selection_generation === selection_generation &&
							Option.getOrUndefined(current.snapshot.selected_group_id) === group_id
								? {
										...current,
										snapshot: {
											...current.snapshot,
											error: Option.some(error.message),
										},
									}
								: current,
						),
					onSuccess: (orchestration_graph) =>
						Update((current) =>
							current.selection_generation === selection_generation &&
							Option.getOrUndefined(current.snapshot.selected_group_id) === group_id
								? {
										...current,
										snapshot: {
											...current.snapshot,
											orchestration_graph: Option.some(orchestration_graph),
										},
									}
								: current,
						),
				}),
			);

		const StartSelectedSubscriptions = (thread_id: string, selection_generation: number) =>
			Effect.gen(function* () {
				const previous = yield* Ref.get(selected_subscription_fibers);
				yield* Effect.forEach(previous, Fiber.interrupt, { discard: true });
				const is_current = () =>
					Effect.map(SubscriptionRef.get(state), (current) =>
						IsCurrentThreadSelection(
							current.snapshot,
							current.selection_generation,
							thread_id,
							selection_generation,
						),
					);
				const start = <Update>(
					subscribe: Effect.Effect<
						Stream.Stream<Update, { readonly message: string }>,
						{ readonly message: string },
						Scope.Scope
					>,
					apply: (
						snapshot: LiveWorkspaceSnapshot,
						update: Update,
					) => LiveWorkspaceSnapshot,
					surface: string,
				) =>
					RunProjectionSubscription(
						subscribe,
						(update) =>
							is_current().pipe(
								Effect.flatMap((current) =>
									current ? UpdateState(apply, update) : Effect.void,
								),
							),
						(message) =>
							is_current().pipe(
								Effect.flatMap((current) =>
									current
										? Update((state) => ({
												...state,
												snapshot: {
													...state.snapshot,
													errors: {
														...state.snapshot.errors,
														[surface]: message,
													},
												},
											}))
										: Effect.void,
								),
							),
					);
				const UpdateState = <Update>(
					apply: (
						snapshot: LiveWorkspaceSnapshot,
						update: Update,
					) => LiveWorkspaceSnapshot,
					update: Update,
				) =>
					Update((current) => ({
						...current,
						snapshot: apply(current.snapshot, update),
					}));
				const current_work = Option.getOrUndefined(
					(yield* SubscriptionRef.get(state)).snapshot.thread_work,
				);
				const usage_subscription =
					current_work === undefined
						? []
						: [
								Effect.forkIn(
									Effect.scoped(
										start(
											client.SubscribeSurfaceUsageAggregate({
												scope: "run",
												scope_id: current_work.run_id,
											}),
											(snapshot, update) => ({
												...snapshot,
												surface_usage: Option.some(update.snapshot),
											}),
											"surface_usage",
										),
									),
									scope,
								),
							];
				const fibers = yield* Effect.all(
					[
						Effect.forkIn(
							Effect.scoped(
								start(
									client.SubscribeThreadTranscript(thread_id),
									ApplyThreadTranscriptUpdate,
									"transcript",
								),
							),
							scope,
						),
						Effect.forkIn(
							Effect.scoped(
								start(
									client.SubscribeThreadSession(thread_id),
									(snapshot, update) => ({
										...snapshot,
										session: Option.some(update.snapshot),
									}),
									"session",
								),
							),
							scope,
						),
						Effect.forkIn(
							Effect.scoped(
								start(
									client.SubscribeOrchestrationGroups(thread_id, true),
									(snapshot, update) => ({
										...snapshot,
										orchestration_groups: Option.some(update.snapshot),
									}),
									"groups",
								),
							),
							scope,
						),
						Effect.forkIn(
							Effect.scoped(
								start(
									client.SubscribeWorkspaceConflicts(thread_id),
									(snapshot, update) => ({
										...snapshot,
										workspace_conflicts: Option.some(update.snapshot),
									}),
									"conflicts",
								),
							),
							scope,
						),
						Effect.forkIn(
							Effect.scoped(
								start(
									client.SubscribeSurfaceItems({ thread_id }),
									(snapshot, update) => ({
										...snapshot,
										surface_items: Option.some(update.snapshot),
									}),
									"surface_items",
								),
							),
							scope,
						),
						...usage_subscription,
					],
					{ concurrency: "unbounded" },
				);
				yield* Ref.set(selected_subscription_fibers, fibers);
			});

		/** Hydrates a new authoritative selection after either a query or stream snapshot. */
		const HydrateSelectedThread = (thread_id: string, selection_generation: number) =>
			Effect.gen(function* () {
				yield* LoadSelectedThread(thread_id, selection_generation);
				yield* LoadThreadSurfaces(thread_id, selection_generation);
				yield* StartSelectedSubscriptions(thread_id, selection_generation);
				const group_id = Option.getOrUndefined(
					(yield* SubscriptionRef.get(state)).snapshot.selected_group_id,
				);
				if (group_id !== undefined) yield* LoadGraph(group_id, selection_generation);
			});

		const Refresh = Effect.gen(function* () {
			const started = yield* UpdateAndGet((current) => ({
				...current,
				refresh_generation: current.refresh_generation + 1,
			}));
			const refresh_generation = started.refresh_generation;
			const list_generation = started.list_generation;

			yield* client.ListThreads.pipe(
				Effect.matchEffect({
					onFailure: (error) =>
						Update((current) =>
							current.refresh_generation === refresh_generation
								? {
										...current,
										snapshot: {
											...current.snapshot,
											error: Option.some(error.message),
											phase: "error",
										},
									}
								: current,
						),
					onSuccess: (threads) =>
						Effect.gen(function* () {
							const selected_thread_id = Option.getOrUndefined(
								(yield* SubscriptionRef.get(state)).snapshot.selected_thread_id,
							);
							const refreshed_state = yield* UpdateAndGet((current) => {
								if (
									current.refresh_generation !== refresh_generation ||
									current.list_generation !== list_generation
								) {
									return current;
								}

								const refreshed = ApplyAuthoritativeThreadRefresh(
									current.snapshot,
									threads,
								);
								const selection_changed =
									Option.getOrUndefined(current.snapshot.selected_thread_id) !==
									Option.getOrUndefined(refreshed.selected_thread_id);

								return {
									...current,
									selection_generation: selection_changed
										? current.selection_generation + 1
										: current.selection_generation,
									snapshot: refreshed,
								};
							});
							if (
								Option.getOrUndefined(
									refreshed_state.snapshot.selected_thread_id,
								) !== selected_thread_id
							)
								yield* StopTerminalOutputWatchers();
						}),
				}),
			);

			const current = yield* SubscriptionRef.get(state);
			const selected_thread_id = Option.getOrUndefined(current.snapshot.selected_thread_id);
			if (
				selected_thread_id !== undefined &&
				current.refresh_generation === refresh_generation
			) {
				yield* HydrateSelectedThread(selected_thread_id, current.selection_generation);
			}
			yield* client.GetGlobalGuidance.pipe(
				Effect.tap((global_guidance) =>
					Update((current) =>
						current.refresh_generation === refresh_generation
							? {
									...current,
									snapshot: {
										...current.snapshot,
										global_guidance: Option.some(global_guidance),
									},
								}
							: current,
					),
				),
				Effect.ignore,
			);
			yield* client.GetModelBehaviour.pipe(
				Effect.tap((model_behaviour) =>
					Update((current) =>
						current.refresh_generation === refresh_generation
							? {
									...current,
									snapshot: {
										...current.snapshot,
										model_behaviour: Option.some(model_behaviour),
									},
								}
							: current,
					),
				),
				Effect.ignore,
			);
		});

		const SelectThread = (thread_id: string) =>
			Effect.gen(function* () {
				yield* StopTerminalOutputWatchers();
				const selected = yield* UpdateAndGet((current) => ({
					...current,
					selection_generation: current.selection_generation + 1,
					snapshot: SelectThreadSnapshot(current.snapshot, thread_id),
				}));
				yield* LoadSelectedThread(thread_id, selected.selection_generation);
				yield* LoadThreadSurfaces(thread_id, selected.selection_generation);
				yield* StartSelectedSubscriptions(thread_id, selected.selection_generation);
				const project = selected.snapshot.threads.find(
					(thread) => thread.thread_id === thread_id,
				)?.primary_project;
				if (project !== undefined) {
					yield* Effect.all(
						[
							RefreshWorkspaceFiles({ workspace_id: project.project_id }),
							RefreshWorkspaceChanges({
								thread_id,
								workspace_id: project.project_id,
							}),
							RefreshGitWorkspace({ thread_id, workspace_id: project.project_id }),
							RefreshTerminals(thread_id, project.project_id),
						],
						{ concurrency: "unbounded" },
					);
				}
			});

		const SelectOrchestrationGroup = (group_id: string) =>
			Effect.gen(function* () {
				const previous = yield* Ref.get(group_subscription_fiber);
				if (Option.isSome(previous)) yield* Fiber.interrupt(previous.value);
				const selected = yield* UpdateAndGet((current) => ({
					...current,
					selection_generation: current.selection_generation + 1,
					snapshot: {
						...current.snapshot,
						selected_group_id: Option.some(group_id),
						orchestration_graph: Option.none(),
					},
				}));
				yield* LoadGraph(group_id, selected.selection_generation);
				const subscription = RunProjectionSubscription(
					client.SubscribeOrchestrationGraph(group_id),
					(update) =>
						Update((current) =>
							current.selection_generation === selected.selection_generation &&
							Option.getOrUndefined(current.snapshot.selected_group_id) === group_id
								? {
										...current,
										snapshot: {
											...current.snapshot,
											orchestration_graph: Option.some(update.graph),
										},
									}
								: current,
						),
					(message) =>
						Update((current) => ({
							...current,
							snapshot: {
								...current.snapshot,
								errors: { ...current.snapshot.errors, graph: message },
							},
						})),
				);
				yield* Effect.forkIn(Effect.scoped(subscription), scope).pipe(
					Effect.tap((fiber) => Ref.set(group_subscription_fiber, Option.some(fiber))),
				);
			});

		const SendMessage = (text: string) =>
			Effect.gen(function* () {
				const snapshot = (yield* SubscriptionRef.get(state)).snapshot;
				const command = BuildLiveWorkspaceMessageCommand(snapshot, text);

				if (Option.isNone(command)) {
					yield* Update((current) => ({
						...current,
						snapshot: {
							...current.snapshot,
							error: Option.some(
								"A selected thread, project, and Codex-only message policy are required.",
							),
						},
					}));
					return;
				}

				yield* client.Command(command.value).pipe(
					Effect.matchEffect({
						onFailure: (error) =>
							Update((current) => ({
								...current,
								snapshot: {
									...current.snapshot,
									error: Option.some(error.message),
								},
							})),
						onSuccess: () =>
							Update((current) => ({
								...current,
								snapshot: { ...current.snapshot, error: Option.none() },
							})),
					}),
				);
			});

		const CreateThread = (title: string) =>
			Effect.gen(function* () {
				const trimmed = title.trim();
				const thread_id = globalThis.crypto?.randomUUID?.();

				if (trimmed.length === 0 || thread_id === undefined) {
					yield* Update((current) => ({
						...current,
						snapshot: {
							...current.snapshot,
							error: Option.some(
								"A secure thread identifier and non-empty title are required.",
							),
						},
					}));
					return;
				}

				yield* client
					.Command({
						payload: { title: trimmed, type: "thread.create" },
						thread_id: `thread_${thread_id}`,
					})
					.pipe(
						Effect.matchEffect({
							onFailure: (error) =>
								Update((current) => ({
									...current,
									snapshot: {
										...current.snapshot,
										error: Option.some(error.message),
									},
								})),
							onSuccess: () =>
								Effect.gen(function* () {
									// The stream remains the live update path, while this authoritative
									// refresh closes the command/subscription race for the creating client.
									yield* Refresh;
									yield* SelectThread(`thread_${thread_id}`);
									yield* Update((current) => ({
										...current,
										snapshot: { ...current.snapshot, error: Option.none() },
									}));
								}),
						}),
					);
			});

		/** Query-only projections are explicitly replaced after a successful mutation. */
		const RefreshWorkspaceFiles = (input: ArtisanWorkspaceFileDiscoveryInput) =>
			Project(
				"workspace_files",
				client.ListWorkspaceFiles(input),
				(snapshot, workspace_file_page) => ({
					...snapshot,
					workspace_file_page: Option.some(workspace_file_page),
				}),
			);
		const ReadWorkspaceFile = (input: ArtisanWorkspaceFileReadInput) =>
			client.ReadWorkspaceFile(input).pipe(
				Effect.matchEffect({
					onFailure: (error) =>
						Update((current) => ({
							...current,
							snapshot: {
								...current.snapshot,
								errors: {
									...current.snapshot.errors,
									workspace_file: error.message,
								},
							},
						})).pipe(Effect.as(Option.none<WorkspaceFileReadQueryResult>())),
					onSuccess: (workspace_file) =>
						Update((current) => {
							const { workspace_file: _cleared, ...errors } = current.snapshot.errors;
							return {
								...current,
								snapshot: {
									...current.snapshot,
									errors,
									workspace_file: Option.some(workspace_file),
								},
							};
						}).pipe(Effect.as(Option.some(workspace_file))),
				}),
			);
		const ReplaceWorkspaceFile = (input: ArtisanWorkspaceFileReplaceInput) =>
			client.ReplaceWorkspaceFile(input).pipe(
				Effect.matchEffect({
					onFailure: (replace_error) =>
						ReadWorkspaceFile(input).pipe(
							Effect.map(
								Option.match({
									onNone: (): LiveWorkspaceFileReplaceOutcome => ({
										_tag: "Failed",
										message: replace_error.message,
									}),
									onSome: (file): LiveWorkspaceFileReplaceOutcome => ({
										_tag: "Conflict",
										file,
										message: replace_error.message,
									}),
								}),
							),
						),
					onSuccess: () =>
						Effect.all([
							ReadWorkspaceFile(input),
							RefreshWorkspaceFiles({ workspace_id: input.workspace_id }),
						]).pipe(
							Effect.map(([file]) =>
								Option.match(file, {
									onNone: (): LiveWorkspaceFileReplaceOutcome => ({
										_tag: "Failed",
										message:
											"The file was replaced but its new revision could not be read.",
									}),
									onSome: (read): LiveWorkspaceFileReplaceOutcome => ({
										_tag: "Saved",
										file: read,
									}),
								}),
							),
						),
				}),
			);
		const RefreshWorkspaceChanges = (input: ArtisanWorkspaceChangeListInput) =>
			Project(
				"workspace_changes",
				client.ListWorkspaceChanges(input),
				(snapshot, workspace_changes) => ({
					...snapshot,
					workspace_changes: Option.some(workspace_changes),
				}),
			);
		const LoadWorkspaceChangeDiff = (input: ArtisanWorkspaceChangeDiffInput) =>
			Project(
				"workspace_diff",
				client.GetWorkspaceChangeDiff(input),
				(snapshot, workspace_change_diff) => ({
					...snapshot,
					workspace_change_diff: Option.some(workspace_change_diff),
				}),
			);
		const ReviewWorkspaceChange = (input: ArtisanWorkspaceChangeReviewInput) =>
			Mutate(
				"workspace_review",
				client.ReviewWorkspaceChange(input),
				RefreshWorkspaceChanges({ thread_id: input.thread_id }),
			);
		const RollbackWorkspaceChange = (input: ArtisanWorkspaceChangeRollbackInput) =>
			Mutate(
				"workspace_rollback",
				client.RollbackWorkspaceChange(input),
				RefreshWorkspaceChanges({ thread_id: input.thread_id }),
			);
		const RefreshGitWorkspace = (input: ArtisanGitWorkspaceInput) =>
			Project("git_workspace", client.GetGitWorkspace(input), (snapshot, git_workspace) => ({
				...snapshot,
				git_workspace: Option.some(git_workspace),
			}));
		const RefreshGitDiff = (input: ArtisanGitDiffInput) =>
			Project("git_diff", client.GetGitDiff(input), (snapshot, git_diff) => ({
				...snapshot,
				git_diff: Option.some(git_diff),
			}));
		const RequestGitIndexMutation = (input: ArtisanGitIndexMutationInput) =>
			Mutate(
				"git_mutation",
				client.RequestGitIndexMutation(input),
				RefreshGitWorkspace({
					thread_id: input.thread_id,
					workspace_id: input.workspace_id,
				}),
			);
		const ResolveGitMutation = (input: ArtisanGitMutationResolveInput) =>
			Mutate("git_mutation", client.ResolveGitMutation(input));
		const RefreshTerminals = (thread_id: string, workspace_id: string) =>
			client.ListTerminals(thread_id, workspace_id).pipe(
				Effect.matchEffect({
					onFailure: (error) =>
						Update((current) => ({
							...current,
							snapshot: {
								...current.snapshot,
								errors: { ...current.snapshot.errors, terminals: error.message },
							},
						})),
					onSuccess: (terminals) =>
						Effect.gen(function* () {
							yield* Update((current) => {
								const { terminals: _cleared, ...errors } = current.snapshot.errors;
								return {
									...current,
									snapshot: { ...current.snapshot, errors, terminals },
								};
							});
							yield* StopTerminalOutputWatchers(
								(watcher) =>
									watcher.thread_id === thread_id &&
									watcher.workspace_id === workspace_id &&
									!terminals.some(
										(terminal) => terminal.terminal_id === watcher.terminal_id,
									),
							);
						}),
				}),
			);
		const WatchTerminalOutput = (terminal_id: string) =>
			Effect.gen(function* () {
				const current = yield* SubscriptionRef.get(state);
				const thread_id = Option.getOrUndefined(current.snapshot.selected_thread_id);
				const terminal = current.snapshot.terminals.find(
					(candidate) =>
						candidate.terminal_id === terminal_id && candidate.thread_id === thread_id,
				);
				if (thread_id === undefined || terminal === undefined) return;
				const watcher: TerminalOutputWatcher = {
					fiber: Option.none(),
					selection_generation: current.selection_generation,
					terminal_id,
					thread_id,
					workspace_id: terminal.workspace_id,
				};
				const should_start = yield* Ref.modify(terminal_output_watchers, (watchers) => {
					if (watchers.has(terminal_id)) return [false, watchers] as const;
					return [true, new Map(watchers).set(terminal_id, watcher)] as const;
				});
				if (!should_start) return;

				const decoder = new TextDecoder();
				const Append = (text: string) =>
					text.length === 0
						? Effect.void
						: Update((current) => {
								if (
									!IsCurrentTerminalOutputWatcher(
										current.snapshot,
										current.selection_generation,
										watcher,
									)
								)
									return current;
								const combined = AppendBoundedTerminalOutput(
									current.snapshot.terminal_output[terminal_id] ?? "",
									text,
								);
								return {
									...current,
									snapshot: {
										...current.snapshot,
										terminal_output: {
											...current.snapshot.terminal_output,
											[terminal_id]: combined,
										},
									},
								};
							});

				yield* Update((current) => ({
					...current,
					snapshot: {
						...current.snapshot,
						terminal_output: {
							...current.snapshot.terminal_output,
							[terminal_id]: "",
						},
					},
				}));

				const Subscribe = client
					.OpenTerminalOutput({
						terminal_id,
						thread_id: watcher.thread_id,
						workspace_id: watcher.workspace_id,
					})
					.pipe(
						Effect.flatMap((output) =>
							Stream.runForEach(output, (bytes) =>
								Append(decoder.decode(bytes, { stream: true })),
							),
						),
						Effect.ensuring(Effect.suspend(() => Append(decoder.decode()))),
						Effect.catch((error) =>
							Update((current) => ({
								...current,
								snapshot: {
									...current.snapshot,
									errors: {
										...current.snapshot.errors,
										[`terminal_output:${terminal_id}`]: error.message,
									},
								},
							})),
						),
						Effect.ensuring(ClearTerminalOutputWatcher(watcher)),
					);
				const fiber = yield* Effect.forkIn(Effect.scoped(Subscribe), scope);
				const current_watcher = yield* Ref.get(terminal_output_watchers);
				if (current_watcher.get(terminal_id) !== watcher) {
					yield* Fiber.interrupt(fiber);
					return;
				}
				yield* Ref.set(
					terminal_output_watchers,
					new Map(current_watcher).set(terminal_id, {
						...watcher,
						fiber: Option.some(fiber),
					}),
				);
			});
		const RefreshPreviewTargets = () =>
			Project(
				"preview_targets",
				client.ListPreviewTargets(),
				(snapshot, preview_targets) => ({ ...snapshot, preview_targets }),
			);
		const RefreshTools = (input: ArtisanToolRegistryListInput) =>
			Project("tools", client.ListArtisanTools(input), (snapshot, tool_registry) => ({
				...snapshot,
				tool_registry: Option.some(tool_registry),
			}));
		const RefreshToolInvocations = (input: ArtisanToolInvocationListInput) =>
			Project(
				"tool_invocations",
				client.ListArtisanToolInvocations(input),
				(snapshot, tool_invocations) => ({
					...snapshot,
					tool_invocations: Option.some(tool_invocations),
				}),
			);
		const RefreshToolApprovals = (input: ArtisanApprovalListInput) =>
			Project(
				"tool_approvals",
				client.ListArtisanApprovals(input),
				(snapshot, tool_approvals) => ({
					...snapshot,
					tool_approvals: Option.some(tool_approvals),
				}),
			);
		const ExecuteTool = (input: ArtisanToolExecuteInput) =>
			Mutate(
				"tool_invocation",
				client.ExecuteArtisanTool(input),
				RefreshToolInvocations({ thread_id: input.thread_id }),
			);
		const ResolveToolApproval = (input: ArtisanApprovalResolveInput) =>
			Mutate(
				"tool_approval",
				client.ResolveArtisanApproval(input),
				RefreshToolApprovals({ thread_id: input.thread_id }),
			);
		const RefreshMarketplace = (input: ArtisanMarketplaceBrowseInput) =>
			Effect.all([
				Project("routines", client.ListRoutines(input), (snapshot, routines) => ({
					...snapshot,
					routines: Option.some(routines),
				})),
				Project(
					"capabilities",
					client.ListCapabilities(input),
					(snapshot, capabilities) => ({
						...snapshot,
						capabilities: Option.some(capabilities),
					}),
				),
			]).pipe(Effect.asVoid);
		const GetRoutineDetail = (input: ArtisanRoutineDetailInput) =>
			Project(
				"routine_detail",
				client.GetRoutineDetail(input),
				(snapshot, routine_detail) => ({
					...snapshot,
					routine_detail: Option.some(routine_detail),
				}),
			);
		const GetCapabilityDetail = (input: ArtisanCapabilityDetailInput) =>
			Project(
				"capability_detail",
				client.GetCapabilityDetail(input),
				(snapshot, capability_detail) => ({
					...snapshot,
					capability_detail: Option.some(capability_detail),
				}),
			);
		const GetCapabilityOAuthStatus = (input: ArtisanCapabilityOAuthInput) =>
			Project(
				"capability_oauth",
				client.GetCapabilityOAuthStatus(input),
				(snapshot, capability_oauth) => ({
					...snapshot,
					capability_oauth: Option.some(capability_oauth),
				}),
			);
		const GetPreviewTarget = (input: ArtisanPreviewTargetInput) =>
			Project(
				"preview_target",
				client.GetPreviewTarget(input),
				(snapshot, preview_target) => ({
					...snapshot,
					preview_target: Option.some(preview_target),
				}),
			);
		const GetPreviewAssetMetadata = (input: ArtisanPreviewAssetMetadataInput) =>
			Project(
				"preview_asset_metadata",
				client.GetPreviewAssetMetadata(input),
				(snapshot, preview_asset_metadata) => ({
					...snapshot,
					preview_asset_metadata: Option.some(preview_asset_metadata),
				}),
			);
		const OpenPreviewInspection = (input: ArtisanPreviewInspectionOpenInput) =>
			Project(
				"preview_inspection_session",
				client.OpenPreviewInspectionSession(input),
				(snapshot, preview_inspection_session) => ({
					...snapshot,
					preview_inspection_session: Option.some(preview_inspection_session),
				}),
			);
		const InspectPreview = (input: ArtisanPreviewInspectionInput) =>
			Project(
				"preview_inspection",
				client.InspectPreviewSession(input),
				(snapshot, preview_inspection_result) => ({
					...snapshot,
					preview_inspection_result: Option.some(preview_inspection_result),
				}),
			);
		const Actions: LiveWorkspaceActions = {
			Command: client.Command,
			OpenTerminalOutput: client.OpenTerminalOutput,
			OpenAsset: client.OpenAsset,
			ResolveRichLink: client.ResolveRichLink,
			GetCapabilityOAuthStatus: client.GetCapabilityOAuthStatus,
			RegisterPreviewTarget: client.RegisterPreviewTarget,
			ProbePreviewTarget: client.ProbePreviewTarget,
			RemovePreviewTarget: client.RemovePreviewTarget,
			SetPreviewTargetState: client.SetPreviewTargetState,
			LaunchPreviewInExternalBrowser: client.LaunchPreviewInExternalBrowser,
			ClosePreviewInspectionSession: client.ClosePreviewInspectionSession,
			PreviewRoutineInstall: client.PreviewRoutineInstall,
			RequestRoutineInstall: client.RequestRoutineInstall,
			DecideRoutineInstall: client.DecideRoutineInstall,
			EnableRoutine: client.EnableRoutine,
			DisableRoutine: client.DisableRoutine,
			RemoveRoutine: client.RemoveRoutine,
			RollbackRoutine: client.RollbackRoutine,
			SyncRoutine: client.SyncRoutine,
			ResolveRoutineDrift: client.ResolveRoutineDrift,
			RequestRoutineDriftOverwrite: client.RequestRoutineDriftOverwrite,
			DecideRoutineDriftOverwrite: client.DecideRoutineDriftOverwrite,
			InvokeRoutine: client.InvokeRoutine,
			DiscoverNpxSkills: client.DiscoverNpxSkills,
			ImportNpxSkills: client.ImportNpxSkills,
			PreviewCapabilityConnect: client.PreviewCapabilityConnect,
			RequestCapabilityConnect: client.RequestCapabilityConnect,
			DecideCapabilityConnect: client.DecideCapabilityConnect,
			StartCapability: client.StartCapability,
			ReconnectCapability: client.ReconnectCapability,
			CheckCapabilityHealth: client.CheckCapabilityHealth,
			DisconnectCapability: client.DisconnectCapability,
			RestartCapability: client.RestartCapability,
			UninstallCapability: client.UninstallCapability,
			EnableCapability: client.EnableCapability,
			DisableCapability: client.DisableCapability,
			RemoveCapability: client.RemoveCapability,
			SyncCapability: client.SyncCapability,
			ResolveCapabilityDrift: client.ResolveCapabilityDrift,
			RequestCapabilityDriftOverwrite: client.RequestCapabilityDriftOverwrite,
			DecideCapabilityDriftOverwrite: client.DecideCapabilityDriftOverwrite,
			RequestCapabilityInvocation: client.RequestCapabilityInvocation,
			DecideCapabilityInvocation: client.DecideCapabilityInvocation,
			InvokeCapability: client.InvokeCapability,
			BeginCapabilityOAuth: client.BeginCapabilityOAuth,
			CompleteCapabilityOAuth: client.CompleteCapabilityOAuth,
			RefreshCapabilityOAuth: client.RefreshCapabilityOAuth,
			RevokeCapabilityOAuth: client.RevokeCapabilityOAuth,
			UpdateGlobalGuidance: client.UpdateGlobalGuidance,
			SelectGlobalGuidance: client.SelectGlobalGuidance,
			ResolveGlobalGuidanceDrift: client.ResolveGlobalGuidanceDrift,
			RetryGlobalGuidanceSync: client.RetryGlobalGuidanceSync,
			UpdateModelBehaviour: client.UpdateModelBehaviour,
			ResolveModelBehaviourDrift: client.ResolveModelBehaviourDrift,
			RetryModelBehaviourSync: client.RetryModelBehaviourSync,
			UpdateThreadSessionPolicy: client.UpdateThreadSessionPolicy,
			GetThreadRetentionPolicy: client.GetThreadRetentionPolicy,
			UpdateThreadRetentionPolicy: client.UpdateThreadRetentionPolicy,
		};

		const StartThreadListSubscription = Effect.gen(function* () {
			const previous = yield* Ref.get(subscription_fiber);
			if (Option.isSome(previous)) yield* Fiber.interrupt(previous.value);

			const started = yield* UpdateAndGet((current) => ({
				...current,
				subscription_generation: current.subscription_generation + 1,
			}));
			const subscription_generation = started.subscription_generation;
			const Subscribe = RunThreadListSubscription(
				client.SubscribeThreadList,
				(update) =>
					Effect.gen(function* () {
						const selected_thread_id = Option.getOrUndefined(
							(yield* SubscriptionRef.get(state)).snapshot.selected_thread_id,
						);
						const next = yield* UpdateAndGet((current) => {
							if (current.subscription_generation !== subscription_generation) {
								return current;
							}

							const snapshot = ApplyThreadListUpdate(current.snapshot, update);
							return {
								...current,
								list_generation: current.list_generation + 1,
								selection_generation:
									Option.getOrUndefined(snapshot.selected_thread_id) !==
									Option.getOrUndefined(current.snapshot.selected_thread_id)
										? current.selection_generation + 1
										: current.selection_generation,
								snapshot,
							};
						});
						if (
							Option.getOrUndefined(next.snapshot.selected_thread_id) !==
							selected_thread_id
						) {
							yield* StopTerminalOutputWatchers();
							const next_thread_id = Option.getOrUndefined(
								next.snapshot.selected_thread_id,
							);
							if (next_thread_id !== undefined)
								yield* HydrateSelectedThread(
									next_thread_id,
									next.selection_generation,
								);
						}
					}),
				(message) =>
					Update((current) =>
						current.subscription_generation === subscription_generation
							? {
									...current,
									snapshot: ApplyThreadListSubscriptionFailure(
										current.snapshot,
										message,
									),
								}
							: current,
					),
			);
			const fiber = yield* Effect.forkIn(Effect.scoped(Subscribe), scope);
			yield* Ref.set(subscription_fiber, Option.some(fiber));
		});

		const StopThreadListSubscription = Effect.gen(function* () {
			const previous = yield* Ref.get(subscription_fiber);
			if (Option.isSome(previous)) yield* Fiber.interrupt(previous.value);
			yield* Ref.set(subscription_fiber, Option.none());
			yield* Update((current) => ({
				...current,
				subscription_generation: current.subscription_generation + 1,
			}));
		});

		const HandleConnection = (connection: FrontendConnectionState) =>
			Effect.gen(function* () {
				yield* Update((current) => ({
					...current,
					snapshot: {
						...current.snapshot,
						error:
							connection.message === undefined
								? current.snapshot.error
								: Option.some(connection.message),
						phase: ToLiveWorkspacePhase(connection.phase),
					},
				}));
				if (ShouldRefreshForConnection(connection.phase)) {
					yield* StartThreadListSubscription;
					yield* Effect.forkIn(Refresh, scope);
				} else {
					yield* StopThreadListSubscription;
				}
			});

		const initial_connection = yield* lifecycle.Current;
		yield* HandleConnection(initial_connection);
		yield* Effect.forkIn(Stream.runForEach(lifecycle.Changes, HandleConnection), scope);

		return LiveWorkspaceStore.of({
			Actions,
			Changes: SubscriptionRef.changes(state).pipe(Stream.map((current) => current.snapshot)),
			CreateThread,
			Refresh,
			SendMessage,
			RefreshWorkspaceFiles,
			ReadWorkspaceFile,
			ReplaceWorkspaceFile,
			RefreshWorkspaceChanges,
			LoadWorkspaceChangeDiff,
			ReviewWorkspaceChange,
			RollbackWorkspaceChange,
			RefreshGitWorkspace,
			RefreshGitDiff,
			RequestGitIndexMutation,
			ResolveGitMutation,
			RefreshTerminals,
			WatchTerminalOutput,
			RefreshPreviewTargets,
			RefreshTools,
			RefreshToolInvocations,
			RefreshToolApprovals,
			ExecuteTool,
			ResolveToolApproval,
			RefreshMarketplace,
			GetRoutineDetail,
			GetCapabilityDetail,
			GetCapabilityOAuthStatus,
			GetPreviewTarget,
			GetPreviewAssetMetadata,
			OpenPreviewInspection,
			InspectPreview,
			SelectOrchestrationGroup,
			SelectThread,
			Snapshot: SubscriptionRef.get(state).pipe(Effect.map((current) => current.snapshot)),
		});
	}),
);
