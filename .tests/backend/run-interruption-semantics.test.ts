import { describe, expect, it } from "vitest";

import { engine_exit_is_interruption } from "../../modules/engines/src/process/exit-classification";
import { lifecycle } from "../../modules/backend/src/conversation/projection/domain";
import { ConversationLifecycle } from "../../modules/protocol/src/conversation";

/**
 * A host restart is not an error. These pin the one distinction that decides
 * both whether the reader sees a red failure card and whether the turn can be
 * picked back up — a distinction the durable record always carried and the
 * projection used to discard.
 */
describe("run interruption semantics", () => {
	describe("projection lifecycle", () => {
		/**
		 * The regression this exists for: `interrupted` was folded into `failed`,
		 * so every reboot mid-turn presented as an engine error.
		 */
		it("keeps an interrupted run distinct from a failed one", () => {
			expect(lifecycle("interrupted")).toBe("interrupted");
			expect(lifecycle("failed")).toBe("failed");
		});

		it("leaves every other durable state mapping untouched", () => {
			expect(lifecycle("completed")).toBe("completed");
			expect(lifecycle("closed")).toBe("completed");
			expect(lifecycle("cancelled")).toBe("cancelled");
			expect(lifecycle("waiting")).toBe("waiting");
			expect(lifecycle("pending")).toBe("pending");
			expect(lifecycle("streaming")).toBe("streaming");
			expect(lifecycle("running")).toBe("active");
		});

		it("is a lifecycle the protocol actually admits", () => {
			expect(ConversationLifecycle.literals).toContain("interrupted");
		});
	});

	describe("engine exit classification", () => {
		/**
		 * `signal` is carried by EngineProcessExit and used to be dropped on the
		 * floor, which is the only reason an OS-killed CLI was indistinguishable
		 * from one that crashed.
		 */
		it("reads a signalled death as an interruption", () => {
			expect(engine_exit_is_interruption({ code: null, signal: "SIGTERM" })).toBe(true);
			expect(engine_exit_is_interruption({ code: null, signal: "SIGKILL" })).toBe(true);
		});

		it("reads an exit with no code at all as an interruption", () => {
			expect(engine_exit_is_interruption({ code: null, signal: null })).toBe(true);
		});

		/** A CLI that chose its own exit code failed; it was not killed. */
		it("leaves an ordinary non-zero exit classified as a failure", () => {
			expect(engine_exit_is_interruption({ code: 1, signal: null })).toBe(false);
			expect(engine_exit_is_interruption({ code: 0, signal: null })).toBe(false);
		});
	});
});
