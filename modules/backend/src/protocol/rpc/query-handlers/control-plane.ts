import { Effect } from "effect";

import type {
	ArtisanApprovalListQueryEnvelope,
	ArtisanToolInvocationListQueryEnvelope,
	ArtisanToolRegistryListQueryEnvelope,
	OrchestrationGraphQueryEnvelope,
	OrchestrationGroupListQueryEnvelope,
	SurfaceListQueryEnvelope,
	SurfaceUsageAggregateQueryEnvelope,
	SurfaceUsageDailyQueryEnvelope,
	TerminalListQueryEnvelope,
} from "@artisan/protocol";

import { AgentGraphOrchestrator } from "../../../orchestration/agent-graph-orchestrator";
import { RuntimeMetadata } from "../../../runtime/metadata";
import { SurfaceService } from "../../../surfaces/service";
import { TerminalSessionService } from "../../../terminal/sessions";
import { ToolControlPlane } from "../../../tools/tool-control-plane";
import type { ReadyState } from "../../connection-state";
import { ConnectionResponseSink } from "./project";

export const MakeControlPlaneHandlers = Effect.gen(function* () {
	const graph = yield* AgentGraphOrchestrator;
	const metadata = yield* RuntimeMetadata;
	const surfaces = yield* SurfaceService;
	const terminals = yield* TerminalSessionService;
	const tools = yield* ToolControlPlane;
	const { Enqueue, EnqueueError } = yield* ConnectionResponseSink;

	const HandleTerminalListQuery = (query: TerminalListQueryEnvelope, current: ReadyState) =>
		terminals.List(query.payload.thread_id, query.payload.workspace_id).pipe(
			Effect.flatMap((terminals) =>
				Effect.gen(function* () {
					const message_id = yield* metadata.MakeId("message");
					const sent_at = yield* metadata.Now;

					yield* Enqueue({
						correlation_id: query.message_id,
						kind: "terminal.list.query.result",
						message_id,
						origin: "backend",
						payload: { terminals },
						protocol_version: 1,
						schema_version: 1,
						sent_at,
					});
				}),
			),
			Effect.catch(() =>
				EnqueueError(
					current,
					"projection.unavailable",
					"The terminal projection could not be read.",
					true,
					query.message_id,
				),
			),
		);

	const HandleGraphQuery = (query: OrchestrationGraphQueryEnvelope, current: ReadyState) =>
		graph.GetGraph(query.payload.group_id).pipe(
			Effect.flatMap((projection) =>
				Effect.gen(function* () {
					const message_id = yield* metadata.MakeId("message");
					const sent_at = yield* metadata.Now;

					yield* Enqueue({
						correlation_id: query.message_id,
						kind: "orchestration.graph.query.result",
						message_id,
						origin: "backend",
						payload: { graph: projection },
						protocol_version: 1,
						schema_version: 1,
						sent_at,
					});
				}),
			),
			Effect.catch(() =>
				EnqueueError(
					current,
					"projection.unavailable",
					"The orchestration graph projection could not be read.",
					true,
					query.message_id,
				),
			),
		);

	const HandleGroupListQuery = (
		query: OrchestrationGroupListQueryEnvelope,
		current: ReadyState,
	) =>
		graph.ListGroupsSnapshot(query.payload.thread_id, query.payload.include_terminal).pipe(
			Effect.flatMap((snapshot) =>
				Effect.gen(function* () {
					const message_id = yield* metadata.MakeId("message");
					const sent_at = yield* metadata.Now;
					yield* Enqueue({
						correlation_id: query.message_id,
						kind: "orchestration.group.list.query.result",
						message_id,
						origin: "backend",
						payload: snapshot,
						protocol_version: 1,
						schema_version: 1,
						sent_at,
					});
				}),
			),
			Effect.catch(() =>
				EnqueueError(
					current,
					"projection.unavailable",
					"The orchestration group list could not be read.",
					true,
					query.message_id,
				),
			),
		);

	const HandleSurfaceListQuery = (query: SurfaceListQueryEnvelope, current: ReadyState) =>
		surfaces
			.List({
				thread_id: query.payload.thread_id,
				...(query.payload.run_id === undefined ? {} : { run_id: query.payload.run_id }),
				...(query.payload.group_id === undefined
					? {}
					: { group_id: query.payload.group_id }),
			})
			.pipe(
				Effect.flatMap((snapshot) =>
					Effect.gen(function* () {
						const message_id = yield* metadata.MakeId("message");
						const sent_at = yield* metadata.Now;
						yield* Enqueue({
							correlation_id: query.message_id,
							kind: "surface.list.query.result",
							message_id,
							origin: "backend",
							payload: snapshot,
							protocol_version: 1,
							schema_version: 1,
							sent_at,
						});
					}),
				),
				Effect.catch(() =>
					EnqueueError(
						current,
						"projection.unavailable",
						"Surface items could not be read.",
						true,
						query.message_id,
					),
				),
			);

	const HandleSurfaceUsageQuery = (
		query: SurfaceUsageAggregateQueryEnvelope,
		current: ReadyState,
	) =>
		surfaces.AggregateUsageSnapshot(query.payload).pipe(
			Effect.flatMap((snapshot) =>
				Effect.gen(function* () {
					const message_id = yield* metadata.MakeId("message");
					const sent_at = yield* metadata.Now;
					yield* Enqueue({
						correlation_id: query.message_id,
						kind: "surface.usage.aggregate.query.result",
						message_id,
						origin: "backend",
						payload: snapshot,
						protocol_version: 1,
						schema_version: 1,
						sent_at,
					});
				}),
			),
			Effect.catch(() =>
				EnqueueError(
					current,
					"projection.unavailable",
					"Surface usage could not be read.",
					true,
					query.message_id,
				),
			),
		);

	const HandleSurfaceUsageDailyQuery = (
		query: SurfaceUsageDailyQueryEnvelope,
		current: ReadyState,
	) =>
		surfaces.DailyUsageSnapshot(query.payload).pipe(
			Effect.flatMap((snapshot) =>
				Effect.gen(function* () {
					const message_id = yield* metadata.MakeId("message");
					const sent_at = yield* metadata.Now;
					yield* Enqueue({
						correlation_id: query.message_id,
						kind: "surface.usage.daily.query.result",
						message_id,
						origin: "backend",
						payload: snapshot,
						protocol_version: 1,
						schema_version: 1,
						sent_at,
					});
				}),
			),
			Effect.catch(() =>
				EnqueueError(
					current,
					"projection.unavailable",
					"Daily surface usage could not be read.",
					true,
					query.message_id,
				),
			),
		);

	const HandleToolRegistryQuery = (
		query: ArtisanToolRegistryListQueryEnvelope,
		current: ReadyState,
	) =>
		tools.Registry(query.payload).pipe(
			Effect.flatMap((payload) =>
				Effect.gen(function* () {
					yield* Enqueue({
						correlation_id: query.message_id,
						kind: "artisan.tool.registry.list.query.result",
						message_id: yield* metadata.MakeId("message"),
						origin: "backend",
						payload,
						protocol_version: 1,
						schema_version: 1,
						sent_at: yield* metadata.Now,
					});
				}),
			),
			Effect.catch(() =>
				EnqueueError(
					current,
					"artisan.tool.unavailable",
					"The built-in tool registry is unavailable.",
					true,
					query.message_id,
				),
			),
		);
	const HandleToolInvocationQuery = (
		query: ArtisanToolInvocationListQueryEnvelope,
		current: ReadyState,
	) =>
		tools.Invocations(query.payload).pipe(
			Effect.flatMap((payload) =>
				Effect.gen(function* () {
					yield* Enqueue({
						correlation_id: query.message_id,
						kind: "artisan.tool.invocation.list.query.result",
						message_id: yield* metadata.MakeId("message"),
						origin: "backend",
						payload,
						protocol_version: 1,
						schema_version: 1,
						sent_at: yield* metadata.Now,
					});
				}),
			),
			Effect.catch(() =>
				EnqueueError(
					current,
					"artisan.tool.unavailable",
					"Tool invocation history is unavailable.",
					true,
					query.message_id,
				),
			),
		);
	const HandleToolApprovalQuery = (
		query: ArtisanApprovalListQueryEnvelope,
		current: ReadyState,
	) =>
		tools.Approvals(query.payload).pipe(
			Effect.flatMap((payload) =>
				Effect.gen(function* () {
					yield* Enqueue({
						correlation_id: query.message_id,
						kind: "artisan.approval.list.query.result",
						message_id: yield* metadata.MakeId("message"),
						origin: "backend",
						payload,
						protocol_version: 1,
						schema_version: 1,
						sent_at: yield* metadata.Now,
					});
				}),
			),
			Effect.catch(() =>
				EnqueueError(
					current,
					"artisan.tool.unavailable",
					"Tool approvals are unavailable.",
					true,
					query.message_id,
				),
			),
		);
	return {
		HandleGraphQuery,
		HandleGroupListQuery,
		HandleSurfaceListQuery,
		HandleSurfaceUsageDailyQuery,
		HandleSurfaceUsageQuery,
		HandleTerminalListQuery,
		HandleToolApprovalQuery,
		HandleToolInvocationQuery,
		HandleToolRegistryQuery,
	};
});
