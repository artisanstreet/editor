import { Schema } from "effect";
import { describe, expect, it } from "vitest";

import { MessageImageAttachmentQuery, MessageImageAttachmentQueryResult } from "@artisan/protocol";

describe("message image attachment protocol", () => {
	it("decodes a bounded, thread-scoped request and exact found result", () => {
		const query = Schema.decodeUnknownSync(MessageImageAttachmentQuery)({
			attachment_id: "attachment_1",
			thread_id: "thread_1",
		});
		const result = Schema.decodeUnknownSync(MessageImageAttachmentQueryResult)({
			attachment: {
				bytes: new Uint8Array([137, 80, 78, 71]),
				id: query.attachment_id,
				media_type: "image/png",
				name: "diagram.png",
				size_bytes: 4,
			},
			status: "found",
		});

		expect(result).toEqual({
			attachment: {
				bytes: new Uint8Array([137, 80, 78, 71]),
				id: "attachment_1",
				media_type: "image/png",
				name: "diagram.png",
				size_bytes: 4,
			},
			status: "found",
		});
	});

	it("models absence without carrying a body", () => {
		expect(
			Schema.decodeUnknownSync(MessageImageAttachmentQueryResult)({ status: "not_found" }),
		).toEqual({
			status: "not_found",
		});
	});
});
