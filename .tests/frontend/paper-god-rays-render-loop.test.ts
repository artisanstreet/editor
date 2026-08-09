import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const workspace = resolve(import.meta.dirname, "../..");
const source = readFileSync(
	resolve(workspace, "modules/frontend/src/routes/components/paper-god-rays.svelte"),
	"utf8",
);

describe("Paper God Rays render loop", () => {
	it("coalesces render signals instead of retaining a frame backlog", () => {
		expect(source).toContain(
			"Queue.sliding<{ readonly immediate: boolean; readonly now: number }>(1)",
		);
		expect(source).not.toContain("Queue.unbounded");
		expect(source).toContain("let pending_config: ShaderConfig | undefined;");
	});

	it("renders only while the document and canvas are visible", () => {
		expect(source).toContain(
			'document.addEventListener("visibilitychange", OnVisibilityChange)',
		);
		expect(source).toContain("new IntersectionObserver");
		expect(source).toContain(
			"const CanRender = () => document_visible && canvas_visible && !context_lost;",
		);
		expect(source).toContain("if (!CanRender() || context_is_lost) return;");
		expect(source).toContain("now - last_animation_frame < 1_000 / 30");
		expect(source).toContain(
			"const render_immediately = immediate || next_config !== undefined;",
		);
		expect(source).toContain("Queue.offerUnsafe(renders, { immediate: false, now: next })");
	});

	it("releases every continuous-render browser resource with the attachment scope", () => {
		expect(source).toContain(
			'document.removeEventListener("visibilitychange", observer.OnVisibilityChange)',
		);
		expect(source).toContain("observer.observer?.disconnect()");
		expect(source).toContain("if (frame_id !== undefined) cancelAnimationFrame(frame_id)");
		expect(source).toContain("yield* Effect.acquireRelease(");
	});

	it("falls back to glass and rebuilds every resource after WebGL context loss", () => {
		const loss_listener_index = source.indexOf(
			'canvas.addEventListener("webglcontextlost", OnContextLost)',
		);
		const shader_compile_index = source.indexOf("const CompileShader");

		expect(source).toContain('canvas.addEventListener("webglcontextlost", OnContextLost)');
		expect(source).toContain(
			'canvas.addEventListener("webglcontextrestored", OnContextRestored)',
		);
		expect(loss_listener_index).toBeGreaterThanOrEqual(0);
		expect(loss_listener_index).toBeLessThan(shader_compile_index);
		expect(source).toContain('event["preventDefault"]()');
		expect(source).toContain('canvas.style.visibility = "hidden"');
		expect(source).toContain('canvas.style.visibility = ""');
		expect(source).toContain(
			"const context_is_lost = yield* RunBrowserDom(() => gl.isContextLost())",
		);
		expect(source).toContain("yield* Effect.scoped(");
		expect(source).toContain("AcquirePaperRays(canvas, RestorePaperRays)");
		expect(source).toContain(
			'canvas.removeEventListener("webglcontextlost", context_events.OnContextLost)',
		);
		expect(source).toContain(
			'canvas.removeEventListener("webglcontextrestored", context_events.OnContextRestored)',
		);
	});
});
