import { readFile } from "node:fs/promises";

import { describe, expect, it } from "vitest";

const dispatcher_path = "modules/backend/src/persistence/orchestration/command-dispatch.ts";

describe("run question-answer validation query bounds", () => {
	it("uses one constrained set query for any number of answer keys", async () => {
		const source = await readFile(dispatcher_path, "utf8");
		const start = source.indexOf('if (payload.type === "run.respond_question")');
		const end = source.indexOf("if (send_message && intake)", start);
		expect(start).toBeGreaterThanOrEqual(0);
		expect(end).toBeGreaterThan(start);
		const validation = source.slice(start, end);

		expect(validation.match(/\.select\(/g) ?? []).toHaveLength(1);
		expect(validation).toMatch(
			/inArray\(\s*OrchestrationInteractions\.interaction_id,\s*unique_question_ids,?\s*\)/,
		);
		expect(validation).toContain('eq(OrchestrationInteractions.kind, "question")');
		expect(validation).toContain("eq(OrchestrationInteractions.run_id, run_id)");
		expect(validation).toContain('eq(OrchestrationInteractions.state, "requested")');
	});

	it("preserves first-invalid input order and rejects anomalous duplicate rows", async () => {
		const source = await readFile(dispatcher_path, "utf8");
		const start = source.indexOf('if (payload.type === "run.respond_question")');
		const end = source.indexOf("if (send_message && intake)", start);
		const validation = source.slice(start, end);

		expect(validation).toContain("for (const question_id of question_ids)");
		expect(validation).toContain("if (!found_question_ids.has(question_id))");
		expect(validation).toContain("interactions.length !== found_question_ids.size");
	});
});
