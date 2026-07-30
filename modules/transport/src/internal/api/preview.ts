import { Effect } from "effect";

import {
	type PreviewAssetMetadataQueryEnvelope,
	type PreviewBrowserLaunchEnvelope,
	type PreviewInspectionEnvelope,
	type PreviewInspectionSessionCloseEnvelope,
	type PreviewInspectionSessionOpenEnvelope,
	type PreviewTargetGetQueryEnvelope,
	type PreviewTargetListQueryEnvelope,
	type PreviewTargetProbeEnvelope,
	type PreviewTargetRegisterEnvelope,
	type PreviewTargetRemoveEnvelope,
	type PreviewTargetStateEnvelope,
	type RichLinkResolveQueryEnvelope,
} from "@artisan/protocol";

import type {
	ArtisanPreviewAssetMetadataInput,
	ArtisanPreviewInspectionInput,
	ArtisanPreviewInspectionOpenInput,
	ArtisanPreviewTargetInput,
	ArtisanPreviewTargetRegistrationInput,
	ArtisanPreviewTargetStateInput,
	ArtisanRichLinkResolveInput,
} from "../../client-api/service";
import { ClientApiContext } from "./context";

type PreviewTargetMutationEnvelope =
	| PreviewTargetRegisterEnvelope
	| PreviewTargetProbeEnvelope
	| PreviewTargetRemoveEnvelope
	| PreviewTargetStateEnvelope;

/** Constructs preview discovery, inspection, and lifecycle operations. */
export const MakePreviewApi = Effect.gen(function* () {
	const context = yield* ClientApiContext;
	const list_preview_targets = (input = {}) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const result = yield* context.Request({
				...trace,
				kind: "preview.target.list.query",
				payload: input,
			} satisfies PreviewTargetListQueryEnvelope);
			return result.kind === "preview.target.list.query.result"
				? result.payload.targets
				: yield* Effect.die("Preview target list response narrowed incorrectly");
		});
	const get_preview_target = (input: ArtisanPreviewTargetInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const result = yield* context.Request({
				...trace,
				kind: "preview.target.get.query",
				payload: input,
			} satisfies PreviewTargetGetQueryEnvelope);
			return result.kind === "preview.target.get.query.result"
				? result.payload
				: yield* Effect.die("Preview target response narrowed incorrectly");
		});
	const get_preview_asset_metadata = (input: ArtisanPreviewAssetMetadataInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const result = yield* context.Request({
				...trace,
				kind: "preview.asset.metadata.query",
				payload: input,
			} satisfies PreviewAssetMetadataQueryEnvelope);
			return result.kind === "preview.asset.metadata.query.result"
				? result.payload
				: yield* Effect.die("Preview asset metadata response narrowed incorrectly");
		});
	const resolve_rich_link = (input: ArtisanRichLinkResolveInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const result = yield* context.Request({
				...trace,
				kind: "preview.rich_link.resolve.query",
				payload: input,
			} satisfies RichLinkResolveQueryEnvelope);
			return result.kind === "preview.rich_link.resolve.query.result"
				? result.payload
				: yield* Effect.die("Rich-link response narrowed incorrectly");
		});
	const mutate_preview_target = (envelope: PreviewTargetMutationEnvelope) =>
		Effect.gen(function* () {
			const result = yield* context.Request(envelope);
			return result.kind === "preview.target.mutation.result"
				? result.payload
				: yield* Effect.die("Preview target mutation response narrowed incorrectly");
		});
	const register_preview_target = (input: ArtisanPreviewTargetRegistrationInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			return yield* mutate_preview_target({
				...trace,
				kind: "preview.target.register",
				payload: input,
			});
		});
	const probe_preview_target = (input: ArtisanPreviewTargetInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			return yield* mutate_preview_target({
				...trace,
				kind: "preview.target.probe",
				payload: input,
			});
		});
	const set_preview_target_state = (input: ArtisanPreviewTargetStateInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			return yield* mutate_preview_target({
				...trace,
				kind: "preview.target.state",
				payload: input,
			});
		});
	const remove_preview_target = (input: ArtisanPreviewTargetInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			return yield* mutate_preview_target({
				...trace,
				kind: "preview.target.remove",
				payload: input,
			});
		});
	const launch_preview_in_external_browser = (input: ArtisanPreviewTargetInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const result = yield* context.Request({
				...trace,
				kind: "preview.browser.launch",
				payload: input,
			} satisfies PreviewBrowserLaunchEnvelope);
			return result.kind === "preview.browser.launch.result"
				? result.payload
				: yield* Effect.die("External-browser launch response narrowed incorrectly");
		});
	const open_preview_inspection_session = (input: ArtisanPreviewInspectionOpenInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const result = yield* context.Request({
				...trace,
				kind: "preview.inspection.open",
				payload: input,
			} satisfies PreviewInspectionSessionOpenEnvelope);
			return result.kind === "preview.inspection.open.result"
				? result.payload
				: yield* Effect.die("Inspection-session open response narrowed incorrectly");
		});
	const inspect_preview_session = (input: ArtisanPreviewInspectionInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const result = yield* context.Request({
				...trace,
				kind: "preview.inspection.inspect",
				payload: input,
			} satisfies PreviewInspectionEnvelope);
			return result.kind === "preview.inspection.inspect.result"
				? result.payload
				: yield* Effect.die("Inspection response narrowed incorrectly");
		});
	const close_preview_inspection_session = (session_id: string) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const result = yield* context.Request({
				...trace,
				kind: "preview.inspection.close",
				payload: { session_id },
			} satisfies PreviewInspectionSessionCloseEnvelope);
			return result.kind === "preview.inspection.close.result"
				? result.payload
				: yield* Effect.die("Inspection-session close response narrowed incorrectly");
		});

	return {
		close_preview_inspection_session,
		get_preview_asset_metadata,
		get_preview_target,
		inspect_preview_session,
		launch_preview_in_external_browser,
		list_preview_targets,
		open_preview_inspection_session,
		probe_preview_target,
		register_preview_target,
		remove_preview_target,
		resolve_rich_link,
		set_preview_target_state,
	};
});
