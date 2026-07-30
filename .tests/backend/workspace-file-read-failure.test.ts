import { Effect, Exit } from "effect";
import { describe, expect, it } from "vitest";

import { WorkspaceErrorDetail } from "../../modules/backend/src/protocol/rpc/query-handlers/workspace-inspection";
import { DecodeWorkspaceText } from "../../modules/backend/src/workspace/files/content";
import { WorkspaceFileServiceError } from "../../modules/backend/src/workspace/files/service";

/**
 * Opening a database, an image, or any other binary must say so. Every read
 * failure used to collapse into one reason, leaving the editor with nothing to
 * show but "the workspace operation could not be completed" — which explains
 * nothing and reads as a bug in the editor rather than a property of the file.
 */
describe("workspace file read failure", () => {
	it("decodes UTF-8 bytes", async () => {
		const exit = await Effect.runPromiseExit(
			DecodeWorkspaceText(new TextEncoder().encode("# readable\n")),
		);

		expect(Exit.isSuccess(exit)).toBe(true);
	});

	/** Lone continuation bytes are invalid UTF-8 in any position. */
	it("refuses bytes that are not UTF-8", async () => {
		const exit = await Effect.runPromiseExit(
			DecodeWorkspaceText(new Uint8Array([0x00, 0xff, 0xfe, 0x80, 0x81])),
		);

		expect(Exit.isFailure(exit)).toBe(true);
	});

	it("reports a non-text read with its own code and reason", () => {
		const detail = WorkspaceErrorDetail(
			new WorkspaceFileServiceError({ operation: "read", reason: "not_text" }),
		);

		expect(detail.code).toBe("workspace.file.not_text");
		expect(detail.message).toContain("not UTF-8 text");
		/** Retrying cannot change a file's bytes, so the editor must not offer to. */
		expect(detail.retryable).toBe(false);
	});

	it("keeps the generic reason for an unexplained failure", () => {
		const detail = WorkspaceErrorDetail(
			new WorkspaceFileServiceError({ operation: "read", reason: "failed" }),
		);

		expect(detail.code).toBe("workspace.unavailable");
	});

	it("keeps the conflict reason for a changed file", () => {
		const detail = WorkspaceErrorDetail(
			new WorkspaceFileServiceError({ operation: "replace", reason: "changed" }),
		);

		expect(detail.code).toBe("workspace.conflict");
	});
});
