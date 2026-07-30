import { Effect, Fiber, Queue } from "effect";

/**
 * How long after a highlight lands the pill keeps verifying its geometry. Long
 * enough to outlast a dropdown's entry animation and any late reflow, short
 * enough to stop well before the next interaction.
 */
const SETTLE_MS = 250;

/**
 * Follows bits' `data-highlighted` marker so a `DropdownHoverSurface` pill tracks
 * the initially highlighted item on open, keyboard arrows, and pointer hover
 * through one signal instead of pointer events alone.
 *
 * On open the marker and the row's final layout do not arrive together: bits
 * highlights the selected row while the content is still mounting, so a single
 * read taken at that moment can capture the row before its padding is in effect
 * and leave the pill short for as long as the dropdown stays open. Rather than
 * guess which frame is trustworthy, this watches for a short window and reveals
 * the pill only once two frames agree on the geometry.
 *
 * @param move_hover - The surface's mover, handed to its children snippet.
 * @returns A Svelte attachment for the highlighted item.
 */
const AcquireHighlight = (move_hover: (event: Event) => void, node: HTMLElement) =>
	Effect.acquireRelease(
		Effect.sync(() => {
			/** The geometry the pill currently sits on; `undefined` until first revealed. */
			let applied: string | undefined = undefined;
			let deadline = 0;
			/** A scheduled frame, or 0 when the watch is idle. */
			let frame = 0;
			/** The previous frame's reading, used to detect that layout has stopped moving. */
			let sampled: string | undefined = undefined;

			const Geometry = () =>
				`${node.offsetTop}:${node.offsetLeft}:${node.offsetWidth}:${node.offsetHeight}`;

			const Apply = (geometry: string) => {
				applied = geometry;
				const event = new Event("highlight");
				Object.defineProperty(event, "currentTarget", { value: node });
				move_hover(event);
			};

			const Settle = () => {
				if (node.dataset.highlighted !== undefined) {
					const geometry = Geometry();
					/**
					 * Before the first reveal, wait for two frames to agree — a pill
					 * revealed mid-layout keeps the half-built size it read. Afterwards,
					 * follow any change, since the row has already proven itself settled.
					 */
					const settled =
						applied === undefined ? geometry === sampled : geometry !== applied;
					if (settled) Apply(geometry);
					sampled = geometry;
				}

				frame = performance.now() < deadline ? requestAnimationFrame(Settle) : 0;
			};

			const Watch = () => {
				deadline = performance.now() + SETTLE_MS;
				if (frame === 0) frame = requestAnimationFrame(Settle);
			};

			const Highlighted = () => {
				if (node.dataset.highlighted === undefined) return;
				/** A highlight arriving after the first reveal lands on settled layout. */
				if (applied !== undefined) Apply(Geometry());
				Watch();
			};

			Watch();

			const highlight_observer = new MutationObserver(Highlighted);
			/**
			 * Border box, not the default content box: the pill is sized from
			 * `offsetHeight`, so a padding change has to count as a resize here.
			 */
			const size_observer = new ResizeObserver(Watch);

			highlight_observer.observe(node, {
				attributeFilter: ["data-highlighted"],
				attributes: true,
			});
			size_observer.observe(node, { box: "border-box" });

			return { frame: () => frame, highlight_observer, size_observer };
		}),
		(resource) =>
			Effect.sync(() => {
				cancelAnimationFrame(resource.frame());
				resource.highlight_observer.disconnect();
				resource.size_observer.disconnect();
			}),
	);

type HighlightCommand =
	| {
			readonly _tag: "Attach";
			readonly id: number;
			readonly move_hover: (event: Event) => void;
			readonly node: HTMLElement;
	  }
	| { readonly _tag: "Release"; readonly id: number };

/**
 * Creates a component-scoped Svelte attachment factory. A single structured
 * worker owns every node fiber; Svelte's synchronous attachment boundary only
 * submits attach/release commands and never creates or runs an Effect scope.
 */
export const MakeFollowHighlight = Effect.gen(function* () {
	const commands = yield* Queue.unbounded<HighlightCommand>();
	let next_id = 0;

	yield* Effect.gen(function* () {
		const highlights = new Map<number, Fiber.Fiber<never>>();

		while (true) {
			const command = yield* Queue.take(commands);
			if (command._tag === "Attach") {
				const fiber = yield* AcquireHighlight(command.move_hover, command.node).pipe(
					Effect.andThen(Effect.never),
					Effect.forkScoped,
				);
				highlights.set(command.id, fiber);
				continue;
			}

			const fiber = highlights.get(command.id);
			if (fiber !== undefined) {
				highlights.delete(command.id);
				yield* Fiber.interrupt(fiber);
			}
		}
	}).pipe(Effect.forkScoped);

	return (move_hover: (event: Event) => void) => (node: HTMLElement) => {
		const id = next_id++;
		Queue.offerUnsafe(commands, { _tag: "Attach", id, move_hover, node });

		return () => {
			Queue.offerUnsafe(commands, { _tag: "Release", id });
		};
	};
});
