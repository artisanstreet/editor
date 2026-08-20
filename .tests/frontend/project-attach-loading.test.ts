import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

import { describe, expect, it } from "vitest";

const { transform_svelte_effect } = await import(
	pathToFileURL(
		resolve(
			process.cwd(),
			"modules/frontend/node_modules/svelte-effect-runtime/.dist/runtime/transform.js",
		),
	).href
);

const picker_source = readFileSync(
	resolve(process.cwd(), "modules/frontend/src/routes/components/project-folder-picker.svelte"),
	"utf8",
);

describe("project attachment loading", () => {
	it("hands opening straight to the system picker, with no card of its own", () => {
		expect(picker_source).toContain("Reset(open).pipe(Effect.forkScoped)");
		expect(picker_source).toContain("client.PickProjectDirectory");
		/** The system dialog is the whole interaction: nothing here paints. */
		expect(picker_source).not.toContain("<Dialog");
		expect(picker_source).not.toContain("Opening the system folder picker");
		expect(picker_source).not.toContain("ListProjectDirectories");
	});

	it("keeps the successful handoff in the component scope when closing reruns Reset", () => {
		const transformed = transform_svelte_effect(
			picker_source,
			"project-folder-picker.svelte",
		).code;

		/**
		 * SER lowers the top-level expression to a reactive launcher. Its cleanup
		 * interrupts that short launcher, while Effect.forkScoped has already put
		 * Reset (and its select → onattached handoff) in the component scope.
		 */
		expect(transformed).toContain("$effect(() => {");
		expect(transformed).toContain("Reset(open).pipe(Effect.forkScoped)");
		expect(transformed).toMatch(/run_scoped\(__SER___scope\.scope, __SER___program\)/u);
	});

	it("fences overlapping picker and attach replies before they publish", () => {
		expect(picker_source).toContain("let request_generation = 0;");
		expect(picker_source).toContain("const generation = BeginRequest();");
		expect(picker_source).toContain("if (!IsCurrentRequest(generation)) return;");
	});

	it("hands a successful attachment directly to the catalog owner without rereading it", () => {
		const attach = picker_source.slice(picker_source.indexOf("const AttachDirectory"));

		expect(attach).toContain("open = false;");
		expect(attach).toContain("yield* onattached(project);");
	});
});
