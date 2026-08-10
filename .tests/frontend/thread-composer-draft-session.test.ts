import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const composer_path = resolve("modules/frontend/src/routes/components/thread-composer.svelte");

describe("thread composer draft session", () => {
	it("reads the reactive draft key inside the stable session factory", () => {
		const source = readFileSync(composer_path, "utf8");
		const factory = source.indexOf("const make_draft_session = () =>");
		const draft_key = source.indexOf("\n\t\t\tdraft_key,", factory);
		const session = source.indexOf("const drafts = yield* make_draft_session();");

		expect(factory).toBeGreaterThan(-1);
		expect(draft_key).toBeGreaterThan(factory);
		expect(session).toBeGreaterThan(draft_key);
		expect(source).not.toContain("const draft_session_options =");
	});
});
