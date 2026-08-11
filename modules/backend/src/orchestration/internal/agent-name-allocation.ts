import { Effect, Random } from "effect";

import { visible_name_maximum } from "./graph-context";

const normalized_name = (name: string) => name.toLowerCase();

const candidate_for = (base: string, generation: number) => {
	if (generation === 1) return base;

	const suffix = ` ${generation}`;
	const bounded_suffix = suffix.slice(-visible_name_maximum);
	const base_length = Math.max(0, visible_name_maximum - bounded_suffix.length);
	return `${base.slice(0, base_length)}${bounded_suffix}`;
};

/**
 * Selects one thread-visible name uniformly without reusing an existing label.
 *
 * Base names are exhausted before suffixes are considered, preserving readable
 * identities while allowing a finite curated bank to serve an unbounded thread.
 */
export const ChooseAvailableAgentName = (
	name_bank: ReadonlyArray<string>,
	existing_display_names: Iterable<string>,
) =>
	Effect.gen(function* () {
		const used_names = new Set([
			"coordinator",
			...Array.from(existing_display_names, normalized_name),
		]);
		const seen_bases = new Set<string>();
		const unique_bases = name_bank.filter((name) => {
			const normalized = normalized_name(name);
			if (seen_bases.has(normalized)) return false;

			seen_bases.add(normalized);
			return true;
		});
		const bases = unique_bases.length > 0 ? unique_bases : ["Agent"];
		let generation = 1;

		while (true) {
			const candidates = bases
				.map((base) => candidate_for(base, generation))
				.filter((candidate) => !used_names.has(normalized_name(candidate)));
			const [first, ...rest] = candidates;

			if (first !== undefined) {
				const index = yield* Random.nextIntBetween(0, rest.length + 1, {
					halfOpen: true,
				});
				if (index === 0) return first;

				const selected = rest[index - 1];
				if (selected !== undefined) return selected;
			}

			generation += 1;
		}
	});
