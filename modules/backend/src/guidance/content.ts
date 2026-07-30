import { basename, extname } from "node:path";

import { Effect, Option, Schema } from "effect";

import { global_guidance_maximum_bytes } from "@artisan/protocol";
import type {
	GlobalGuidanceCandidate,
	GlobalGuidanceProvider,
	GlobalGuidanceProviderMetadata,
} from "@artisan/protocol";

import { GuidanceFileStoreFailure } from "./file-store";
import {
	guidance_hash,
	normalize_guidance_content,
	type GuidanceDiscovery,
	type NativeGuidanceProviderAdapter,
} from "./provider-mirrors";
import { GlobalGuidanceInvariantError } from "./contracts";

export interface CanonicalContent {
	readonly byte_count: number;
	readonly content: string;
	readonly content_hash: string;
}

export interface PresentProvider {
	readonly adapter: NativeGuidanceProviderAdapter;
	readonly discovery: Extract<GuidanceDiscovery, { readonly _tag: "Present" }>;
}

export type ExpectedProviderState =
	| { readonly _tag: "Absent" }
	| { readonly _tag: "Present"; readonly hash: string };

export type ProviderExpectations = ReadonlyMap<GlobalGuidanceProvider, ExpectedProviderState>;

export interface PreparedProviderMutation {
	readonly acceptance: Option.Option<import("./repository").GlobalGuidanceAcceptance>;
	readonly canonical: CanonicalContent;
	readonly request_fingerprint: string;
}

export const ContentMetadata = (content: string) =>
	Effect.gen(function* () {
		const normalized = normalize_guidance_content(content);
		const byte_count = new TextEncoder().encode(normalized).byteLength;

		if (byte_count > global_guidance_maximum_bytes) {
			return yield* new GlobalGuidanceInvariantError({
				operation: "guidance_too_large",
			});
		}

		return {
			byte_count,
			content: normalized,
			content_hash: guidance_hash(normalized),
		} satisfies CanonicalContent;
	});

export function provider_metadata(
	providers: ReadonlyArray<GlobalGuidanceProviderMetadata>,
	provider: GlobalGuidanceProvider,
) {
	return providers.find((entry) => entry.provider === provider);
}

export function reconciliation_operation_id(
	base: string,
	provider: GlobalGuidanceProvider,
	input: Readonly<Record<string, unknown>>,
) {
	const fingerprint = guidance_hash(JSON.stringify(input)).slice(0, 24);
	return `${base}_${provider}_${fingerprint}`;
}

export function provider_mutation_fingerprint(input: Readonly<Record<string, unknown>>) {
	return guidance_hash(JSON.stringify(input));
}

export function make_provider_expectations(
	discoveries: ReadonlyArray<{
		readonly adapter: NativeGuidanceProviderAdapter;
		readonly discovery: GuidanceDiscovery;
	}>,
) {
	return new Map(
		discoveries.flatMap(({ adapter, discovery }) =>
			discovery._tag === "ReadFailed"
				? []
				: [
						[
							adapter.provider,
							discovery._tag === "Present"
								? { _tag: "Present" as const, hash: discovery.hash }
								: { _tag: "Absent" as const },
						] as const,
					],
		),
	) satisfies Map<GlobalGuidanceProvider, ExpectedProviderState>;
}

export function provider_state_matches(
	discovery: Exclude<GuidanceDiscovery, { readonly _tag: "ReadFailed" }>,
	expected: ExpectedProviderState,
) {
	return expected._tag === "Absent"
		? discovery._tag === "Absent"
		: discovery._tag === "Present" && discovery.hash === expected.hash;
}

export function backup_name(label: string, path: string, id: string) {
	const extension = extname(path);
	const stem = basename(path, extension).replace(/[^a-zA-Z0-9._-]/g, "_");
	return `${label}-${stem}-${id}${extension || ".md"}`;
}

const ErrnoCause = Schema.Struct({
	code: Schema.optional(Schema.String),
});

export function guidance_file_error_code(error: GuidanceFileStoreFailure) {
	const code = Schema.decodeUnknownOption(ErrnoCause)(error.cause).pipe(
		Option.flatMap(({ code }) => Option.fromNullishOr(code)),
		Option.getOrUndefined,
	);
	const is_access_error = code === "EACCES" || code === "EBUSY" || code === "EPERM";

	if (error.operation === "restore") {
		return is_access_error ? "guidance_restore_access_denied" : "guidance_restore_failed";
	}
	if (is_access_error) return "guidance_access_denied";
	if (code === "EXDEV") return "guidance_cross_device";
	if (code === "EMLINK" || code === "ENOTSUP") return "guidance_link_unsupported";
	return `guidance_${error.operation}_failed`;
}

export const MakeGuidanceCandidates = (
	discoveries: ReadonlyArray<{
		readonly adapter: NativeGuidanceProviderAdapter;
		readonly discovery: GuidanceDiscovery;
	}>,
) =>
	Effect.forEach(
		discoveries,
		({ adapter, discovery }) =>
			discovery._tag === "Present"
				? ContentMetadata(discovery.content).pipe(
						Effect.map((content) =>
							Option.some({
								byte_count: content.byte_count,
								content_hash: content.content_hash,
								modified_at: discovery.modified_at,
								path: discovery.path,
								preview: content.content,
								provider: adapter.provider,
							} satisfies GlobalGuidanceCandidate),
						),
					)
				: Effect.succeed(Option.none<GlobalGuidanceCandidate>()),
		{ concurrency: "unbounded" },
	).pipe(Effect.map((candidates) => candidates.filter(Option.isSome).map(Option.getOrThrow)));
