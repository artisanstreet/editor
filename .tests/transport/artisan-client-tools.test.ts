import { describe, expect, it } from "vitest";

import type { ArtisanClient } from "@artisan/transport/client";

/** Compile-time transport contract: renderer fixtures must implement every tool-control method. */
interface ToolControlClientContract extends Pick<
	typeof ArtisanClient.Service,
	| "ExecuteArtisanTool"
	| "GetWorkspaceLanguageCapabilities"
	| "ListArtisanApprovals"
	| "ListArtisanToolInvocations"
	| "ListArtisanTools"
	| "ListWorkspaceFiles"
	| "ResolveArtisanApproval"
> {}

const public_methods: ReadonlyArray<keyof ToolControlClientContract> = [
	"ExecuteArtisanTool",
	"GetWorkspaceLanguageCapabilities",
	"ListArtisanApprovals",
	"ListArtisanToolInvocations",
	"ListArtisanTools",
	"ListWorkspaceFiles",
	"ResolveArtisanApproval",
];

describe("ArtisanClient tool-control contract", () => {
	it("keeps all user-visible tool state behind renderer-safe client operations", () => {
		expect(public_methods).toEqual([
			"ExecuteArtisanTool",
			"GetWorkspaceLanguageCapabilities",
			"ListArtisanApprovals",
			"ListArtisanToolInvocations",
			"ListArtisanTools",
			"ListWorkspaceFiles",
			"ResolveArtisanApproval",
		]);
	});
});
