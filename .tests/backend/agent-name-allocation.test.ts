import { Effect, Random } from "effect";
import { describe, expect, it } from "vitest";

import { ChooseAvailableAgentName } from "../../modules/backend/src/orchestration/internal/agent-name-allocation";

const Choose = (name_bank: ReadonlyArray<string>, existing: Iterable<string>, seed: number) =>
	Effect.runPromise(ChooseAvailableAgentName(name_bank, existing).pipe(Random.withSeed(seed)));

describe("agent name allocation", () => {
	it("draws from the full bank instead of taking its first name", async () => {
		const names = await Promise.all(
			Array.from({ length: 24 }, (_, seed) => Choose(["Ada", "Beate", "Cecilie"], [], seed)),
		);

		expect(new Set(names)).toEqual(new Set(["Ada", "Beate", "Cecilie"]));
		expect(names.some((name) => name !== "Ada")).toBe(true);
	});

	it("allocates a random permutation before reusing a base name", async () => {
		const names = await Effect.runPromise(
			Effect.gen(function* () {
				const first = yield* ChooseAvailableAgentName(["Ada", "Beate", "Cecilie"], []);
				const second = yield* ChooseAvailableAgentName(
					["Ada", "Beate", "Cecilie"],
					[first],
				);
				const third = yield* ChooseAvailableAgentName(
					["Ada", "Beate", "Cecilie"],
					[first, second],
				);
				return [first, second, third];
			}).pipe(Random.withSeed("without-replacement")),
		);

		expect(new Set(names)).toEqual(new Set(["Ada", "Beate", "Cecilie"]));
	});

	it("uses a random suffix only after every base name is exhausted", async () => {
		const name = await Choose(["Ada", "Beate"], ["Ada", "Beate"], 17);

		expect(name).toMatch(/^(Ada|Beate) 2$/u);
	});

	it("truncates an exhausted maximum-length base before adding its suffix", async () => {
		const base = "a".repeat(64);
		const name = await Choose([base], [base], 41);

		expect(name).toBe(`${"a".repeat(62)} 2`);
		expect(name).toHaveLength(64);
	});

	it("treats existing labels case-insensitively", async () => {
		const name = await Choose(["Ada", "Beate"], ["aDa"], 31);

		expect(name).toBe("Beate");
	});

	it("keeps the first spelling when a bank repeats a name case-insensitively", async () => {
		const name = await Choose(["Bop", "bop"], ["Bop"], 59);

		expect(name).toBe("Bop 2");
	});

	it("reserves the coordinator label before a new group is persisted", async () => {
		const name = await Choose(["Coordinator"], [], 61);

		expect(name).toBe("Coordinator 2");
	});
});
