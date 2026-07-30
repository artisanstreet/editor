import { Effect } from "effect";
import {
	type ArtisanApprovalResolveEnvelope,
	type ArtisanToolExecuteEnvelope,
} from "@artisan/protocol";
import type {
	ArtisanApprovalResolveInput,
	ArtisanCommandReceipt,
	ArtisanToolExecuteInput,
} from "../../client-api/service";
import { client_error } from "../client-common";
import { ClientApiContext } from "./context";

type ToolMutationEnvelope = ArtisanApprovalResolveEnvelope | ArtisanToolExecuteEnvelope;

/** Constructs Artisan tool execution and approval operations. */
export const MakeToolMutationApi = Effect.gen(function* () {
	const context = yield* ClientApiContext;
	const send = (envelope: ToolMutationEnvelope) =>
		Effect.gen(function* () {
			const result = yield* context.Request(envelope);
			if (result.kind !== "command.receipt") {
				return yield* Effect.die("Artisan tool mutation receipt narrowed incorrectly");
			}
			if (result.payload.status === "rejected") {
				return yield* Effect.fail(
					client_error(
						"protocol",
						result.payload.error.message,
						result.payload.error,
						result.payload.error.retryable,
						result.payload.error.code,
					),
				);
			}
			return {
				command_id: envelope.message_id,
				journal_sequence: result.payload.journal_sequence,
				status: result.payload.status,
			} satisfies ArtisanCommandReceipt;
		});
	const execute_artisan_tool = (input: ArtisanToolExecuteInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			return yield* send({
				...trace,
				kind: "artisan.tool.execute",
				message_id: input.command_id ?? trace.message_id,
				payload: {
					input: input.input,
					invocation_id: input.invocation_id,
					policy: input.policy,
					...(input.raw_origin === undefined ? {} : { raw_origin: input.raw_origin }),
				},
				thread_id: input.thread_id,
				...(input.agent_id === undefined ? {} : { agent_id: input.agent_id }),
				...(input.raw_origin === undefined ? {} : { raw_origin: input.raw_origin }),
				...(input.run_id === undefined ? {} : { run_id: input.run_id }),
			});
		});
	const resolve_artisan_approval = (input: ArtisanApprovalResolveInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			return yield* send({
				...trace,
				kind: "artisan.approval.resolve",
				message_id: input.command_id ?? trace.message_id,
				payload: {
					approval_id: input.approval_id,
					approved: input.approved,
					invocation_id: input.invocation_id,
					resolution_id: input.resolution_id,
				},
				thread_id: input.thread_id,
				...(input.agent_id === undefined ? {} : { agent_id: input.agent_id }),
				...(input.raw_origin === undefined ? {} : { raw_origin: input.raw_origin }),
				...(input.run_id === undefined ? {} : { run_id: input.run_id }),
			});
		});
	return { execute_artisan_tool, resolve_artisan_approval };
});
