import { Effect } from "effect";

import {
	OutboundControlEnvelope,
	type PreviewBrowserLaunchEnvelope,
	type PreviewInspectionEnvelope,
	type PreviewInspectionSessionCloseEnvelope,
	type PreviewInspectionSessionOpenEnvelope,
	type PreviewTargetProbeEnvelope,
	type PreviewTargetRegisterEnvelope,
	type PreviewTargetRemoveEnvelope,
	type PreviewTargetStateEnvelope,
	type ProtocolErrorDetail,
} from "@artisan/protocol";

import { PreviewCoordinator } from "../../../preview/coordinator";
import { PreviewRepositoryError } from "../../../preview/repository";
import { PreviewRuntimeError } from "../../../preview/runtime";
import { PreviewHealthProbeError } from "../../../preview/target";
import { RuntimeMetadata } from "../../../runtime/metadata";
import type { ReadyState } from "../../connection-state";
import { ConnectionResponseSink } from "../query-handlers/project";

const PreviewErrorDetail = (error: unknown): ProtocolErrorDetail => {
	if (error instanceof PreviewRepositoryError) {
		if (error.code === "invalid")
			return {
				code: "preview.invalid",
				message: "The preview request conflicts with the durable preview state.",
				retryable: false,
			};
		if (error.code === "not_found")
			return {
				code: "preview.not_found",
				message: "The requested preview target or inspection session is unavailable.",
				retryable: false,
			};
		return {
			code: "preview.storage_unavailable",
			message: "The preview state could not be durably read or updated.",
			retryable: true,
		};
	}
	if (error instanceof PreviewHealthProbeError)
		return {
			code: "preview.health_unavailable",
			message: "The local preview health probe is currently unavailable.",
			retryable: true,
		};
	if (error instanceof PreviewRuntimeError) {
		if (error.code === "invalid_input" || error.code === "not_found")
			return {
				code: error.code === "invalid_input" ? "preview.invalid" : "preview.not_found",
				message: "The requested preview runtime resource is unavailable.",
				retryable: false,
			};
		if (error.code === "browser_unavailable")
			return {
				code: "preview.browser_unavailable",
				message: "The external browser opener is currently unavailable.",
				retryable: true,
			};
		return {
			code: "preview.connector_unavailable",
			message: "The external preview connector is currently unavailable.",
			retryable: true,
		};
	}
	return {
		code: "preview.unavailable",
		message: "The preview operation could not be completed.",
		retryable: true,
	};
};

export const MakePreviewMutationHandlers = Effect.gen(function* () {
	const previews = yield* PreviewCoordinator;
	const metadata = yield* RuntimeMetadata;
	const { Enqueue, EnqueueError } = yield* ConnectionResponseSink;

	const HandlePreview = <A>(
		envelope: { readonly message_id: string },
		current: ReadyState,
		kind:
			| "preview.asset.metadata.query.result"
			| "preview.browser.launch.result"
			| "preview.inspection.close.result"
			| "preview.inspection.inspect.result"
			| "preview.inspection.open.result"
			| "preview.rich_link.resolve.query.result"
			| "preview.target.get.query.result"
			| "preview.target.list.query.result"
			| "preview.target.mutation.result",
		operation: Effect.Effect<A, unknown, never>,
	) =>
		operation.pipe(
			Effect.flatMap((payload) =>
				Effect.gen(function* () {
					const message_id = yield* metadata.MakeId("message");
					const sent_at = yield* metadata.Now;
					const response = {
						correlation_id: envelope.message_id,
						kind,
						message_id,
						origin: "backend" as const,
						payload,
						protocol_version: 1,
						schema_version: 1,
						sent_at,
					} as OutboundControlEnvelope;
					yield* Enqueue(response);
				}),
			),
			Effect.catch((error) => {
				const detail = PreviewErrorDetail(error);

				return EnqueueError(
					current,
					detail.code,
					detail.message,
					detail.retryable,
					envelope.message_id,
				);
			}),
		);
	const HandlePreviewTargetRegister = (
		command: PreviewTargetRegisterEnvelope,
		current: ReadyState,
	) => {
		const { source, ...registration } = command.payload;
		return HandlePreview(
			command,
			current,
			"preview.target.mutation.result",
			previews.Register({
				...registration,
				...(source === undefined ? {} : { source }),
				message_id: command.message_id,
			}),
		);
	};
	const HandlePreviewTargetProbe = (command: PreviewTargetProbeEnvelope, current: ReadyState) =>
		HandlePreview(
			command,
			current,
			"preview.target.mutation.result",
			previews.Probe({
				message_id: command.message_id,
				target_id: command.payload.target_id,
			}),
		);
	const HandlePreviewTargetState = (command: PreviewTargetStateEnvelope, current: ReadyState) => {
		const requested_state = command.payload.state;
		return requested_state === "removed"
			? EnqueueError(
					current,
					"preview.invalid_state",
					"Use the dedicated preview removal command.",
					false,
					command.message_id,
				)
			: previews.Get(command.payload.target_id).pipe(
					Effect.flatMap((target) =>
						HandlePreview(
							command,
							current,
							"preview.target.mutation.result",
							previews.SetState({
								message_id: command.message_id,
								state: requested_state,
								target_id: command.payload.target_id,
								thread_id: target.thread_id,
							}),
						),
					),
				);
	};
	const HandlePreviewTargetRemove = (command: PreviewTargetRemoveEnvelope, current: ReadyState) =>
		previews.Get(command.payload.target_id).pipe(
			Effect.flatMap((target) =>
				HandlePreview(
					command,
					current,
					"preview.target.mutation.result",
					previews.Remove({
						message_id: command.message_id,
						target_id: command.payload.target_id,
						thread_id: target.thread_id,
					}),
				),
			),
		);
	const HandlePreviewLaunch = (command: PreviewBrowserLaunchEnvelope, current: ReadyState) =>
		HandlePreview(
			command,
			current,
			"preview.browser.launch.result",
			previews.Launch({
				message_id: command.message_id,
				target_id: command.payload.target_id,
			}),
		);
	const HandlePreviewInspectionOpen = (
		command: PreviewInspectionSessionOpenEnvelope,
		current: ReadyState,
	) =>
		HandlePreview(
			command,
			current,
			"preview.inspection.open.result",
			previews.OpenInspection({
				...command.payload,
				message_id: command.message_id,
			}),
		);
	const HandlePreviewInspection = (command: PreviewInspectionEnvelope, current: ReadyState) =>
		HandlePreview(
			command,
			current,
			"preview.inspection.inspect.result",
			previews.Inspect({ ...command.payload, message_id: command.message_id }),
		);
	const HandlePreviewInspectionClose = (
		command: PreviewInspectionSessionCloseEnvelope,
		current: ReadyState,
	) =>
		HandlePreview(
			command,
			current,
			"preview.inspection.close.result",
			previews.CloseInspection({
				...command.payload,
				message_id: command.message_id,
			}),
		);

	return {
		HandlePreviewInspection,
		HandlePreviewInspectionClose,
		HandlePreviewInspectionOpen,
		HandlePreviewLaunch,
		HandlePreviewTargetProbe,
		HandlePreviewTargetRegister,
		HandlePreviewTargetRemove,
		HandlePreviewTargetState,
	};
});
