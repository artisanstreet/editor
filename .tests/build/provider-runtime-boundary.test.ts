import { describe, expect, it } from "vitest";

import {
	AssertForgeBundleHasNoProviderSdk,
	provider_sdk_bundle_markers,
} from "../../.scripts/build/build-forge-sea";

describe("Forge provider runtime boundary", () => {
	it("permits provider-neutral Artisan adapter vocabulary", () => {
		expect(() =>
			AssertForgeBundleHasNoProviderSdk(
				new TextEncoder().encode("Artisan CLI adapter launches an external ACP provider"),
			),
		).not.toThrow();
	});

	it.each(provider_sdk_bundle_markers)("rejects bundled provider package marker %s", (marker) => {
		expect(() => AssertForgeBundleHasNoProviderSdk(new TextEncoder().encode(marker))).toThrow(
			"Forge bundle contains provider SDK package code",
		);
	});
});
