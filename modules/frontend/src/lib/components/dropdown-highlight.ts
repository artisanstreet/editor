import { Effect, Queue } from "effect";
import { RunBrowserDom } from "$lib/browser/dom";
import { MakeScopedAttachmentRunner } from "$lib/lifecycle/scoped-attachment-runner";

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
	Effect.gen(function* () {
		return yield* Effect.acquireRelease(
			Effect.gen(function* () {
				/** The geometry the pill currently sits on; `undefined` until first revealed. */
				let applied: string | undefined = undefined;
				let deadline = 0;
				/** A scheduled frame, or 0 when the watch is idle. */
				let frame = 0;
				/** The previous frame's reading, used to detect that layout has stopped moving. */
				let sampled: string | undefined = undefined;
				const commands = yield* Queue.unbounded<"highlighted" | "settle" | "watch">();

				const Geometry = () =>
					Effect.gen(function* () {
						return yield* RunBrowserDom(
							() =>
								`${node.offsetTop}:${node.offsetLeft}:${node.offsetWidth}:${node.offsetHeight}`,
						);
					});

				const Apply = (geometry: string) =>
					Effect.gen(function* () {
						applied = geometry;
						yield* RunBrowserDom(() => {
							const event = new Event("highlight");
							Object.defineProperty(event, "currentTarget", { value: node });
							move_hover(event);
						});
					});

				const Settle = () =>
					Effect.gen(function* () {
						if (node.dataset.highlighted !== undefined) {
							const geometry = yield* Geometry();
							/**
							 * Before the first reveal, wait for two frames to agree — a pill
							 * revealed mid-layout keeps the half-built size it read. Afterwards,
							 * follow any change, since the row has already proven itself settled.
							 */
							const settled =
								applied === undefined ? geometry === sampled : geometry !== applied;
							if (settled) yield* Apply(geometry);
							sampled = geometry;
						}

						frame = yield* RunBrowserDom(() =>
							performance.now() < deadline
								? requestAnimationFrame(() => Queue.offerUnsafe(commands, "settle"))
								: 0,
						);
					});

				const Watch = () =>
					Effect.gen(function* () {
						yield* RunBrowserDom(() => {
							deadline = performance.now() + SETTLE_MS;
							if (frame === 0)
								frame = requestAnimationFrame(() =>
									Queue.offerUnsafe(commands, "settle"),
								);
						});
					});

				const Highlighted = () =>
					Effect.gen(function* () {
						if (node.dataset.highlighted === undefined) return;
						/** A highlight arriving after the first reveal lands on settled layout. */
						if (applied !== undefined) yield* Apply(yield* Geometry());
						yield* Watch();
					});

				yield* Effect.gen(function* () {
					while (true) {
						const command = yield* Queue.take(commands);
						if (command === "highlighted") yield* Highlighted();
						else if (command === "settle") yield* Settle();
						else yield* Watch();
					}
				}).pipe(Effect.forkScoped);
				yield* Watch();

				const { highlight_observer, size_observer } = yield* RunBrowserDom(() => {
					const highlight_observer = new MutationObserver(() =>
						Queue.offerUnsafe(commands, "highlighted"),
					);
					/** Border box tracks padding because the pill is sized from offsetHeight. */
					const size_observer = new ResizeObserver(() =>
						Queue.offerUnsafe(commands, "watch"),
					);
					highlight_observer.observe(node, {
						attributeFilter: ["data-highlighted"],
						attributes: true,
					});
					size_observer.observe(node, { box: "border-box" });
					return { highlight_observer, size_observer };
				});

				return { frame: () => frame, highlight_observer, size_observer };
			}),
			(resource) =>
				Effect.gen(function* () {
					yield* RunBrowserDom(() => {
						cancelAnimationFrame(resource.frame());
						resource.highlight_observer.disconnect();
						resource.size_observer.disconnect();
					});
				}),
		);
	});

/**
 * Creates a component-scoped Svelte attachment factory. A single structured
 * worker owns every node fiber; Svelte's synchronous attachment boundary only
 * submits attach/release commands and never creates or runs an Effect scope.
 */
export const MakeFollowHighlight = Effect.gen(function* () {
	const highlight_runner = yield* MakeScopedAttachmentRunner(
		({
			move_hover,
			node,
		}: {
			readonly move_hover: (event: Event) => void;
			readonly node: HTMLElement;
		}) =>
			Effect.gen(function* () {
				yield* AcquireHighlight(move_hover, node);
				yield* Effect.never;
			}),
	);

	return (move_hover: (event: Event) => void) => (node: HTMLElement) =>
		highlight_runner.Attachment({ move_hover, node });
});
