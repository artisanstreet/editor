import { Effect, Option } from "effect";

import type { GlobalGuidanceProvider } from "@artisan/protocol";

import {
	type GuidanceDiscovery,
	type NativeGuidanceProviderAdapter,
	guidance_hash,
	normalize_guidance_content,
} from "../../modules/backend/src/guidance/provider-mirrors";
import { GuidanceFileStore } from "../../modules/backend/src/guidance/file-store";

/** Test-only generic native-file adapter for historical multi-provider service coverage. */
export const make_test_native_guidance_adapter = (provider: GlobalGuidanceProvider, path: string) =>
	Effect.gen(function* () {
		const files = yield* GuidanceFileStore;
		const Discover: Effect.Effect<GuidanceDiscovery> = files.Read(path).pipe(
			Effect.map((file) =>
				Option.match(file, {
					onNone: () => ({ _tag: "Absent" as const, path }),
					onSome: ({ content, modified_at }) => {
						const normalized = normalize_guidance_content(content);
						return {
							_tag: "Present" as const,
							content: normalized,
							hash: guidance_hash(normalized),
							modified_at,
							path,
						};
					},
				}),
			),
			Effect.catch(() => Effect.succeed({ _tag: "ReadFailed" as const, path })),
		);

		return {
			Discover,
			mode: "native_file" as const,
			provider,
		} satisfies NativeGuidanceProviderAdapter;
	});
