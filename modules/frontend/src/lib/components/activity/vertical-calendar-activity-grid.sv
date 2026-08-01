<script lang="ts" effect>
	import { Effect } from "effect";

	import { RunBrowserDom } from "$lib/browser/dom";
	import { EngineMarkClass, UsageSlicePresentationFor } from "$lib/engine/presentation";
	import { MakeScopedAttachmentRunner } from "$lib/lifecycle/scoped-attachment-runner";

	/** One engine and model's share of a day's tokens; absent ids mark unattributed work. */
	export type CalendarActivityEngine = {
		engine_id?: string;
		model_id?: string;
		tokens: number;
	};

	export type CalendarActivity = {
		date: string;
		engines: ReadonlyArray<CalendarActivityEngine>;
		tokens: number;
	};

	type Props = {
		activities: ReadonlyArray<CalendarActivity>;
	};

	type CalendarCell = CalendarActivity & {
		column: number;
		row: number;
	};

	const MaximumCellSize = 12;
	const MinimumCellSize = 3;
	const CellGap = 2;

	let { activities }: Props = $props();
	let host: HTMLDivElement | undefined;
	let canvas: HTMLCanvasElement | undefined;
	let hovered = $state<CalendarCell | undefined>();
	let tooltip_x = $state(0);
	let tooltip_y = $state(0);
	let cells: ReadonlyArray<CalendarCell> = [];
	let cell_size = MaximumCellSize;
	let grid_left = 0;
	let grid_top = 0;
	let column_count = 0;
	let row_count = 0;
	type CalendarObservation =
		| {
				readonly _tag: "Observe";
				readonly host: HTMLDivElement;
				readonly draw_key: string;
			  }
		| { readonly _tag: "Draw" }

	const parse_date = (value: string) => new Date(`${value}T12:00:00`);
	const date_key = (date: Date) => {
		const year = date.getFullYear();
		const month = String(date.getMonth() + 1).padStart(2, "0");
		const day = String(date.getDate()).padStart(2, "0");
		return `${year}-${month}-${day}`;
	};

	const add_days = (date: Date, days: number) => {
		const next = new Date(date);
		next.setDate(next.getDate() + days);
		return next;
	};

	const token_label = (tokens: number) => new Intl.NumberFormat("en", { notation: "compact" }).format(tokens);
	const donut_circumference = 2 * Math.PI * 6;
	const engine_share = (activity: CalendarActivity, slice: CalendarActivityEngine) =>
		activity.tokens === 0 ? 0 : slice.tokens / activity.tokens;

	const Draw = (
		observed_host: HTMLDivElement | undefined,
		observed_canvas: HTMLCanvasElement | undefined,
		next_activities: ReadonlyArray<CalendarActivity>,
	) =>
		Effect.gen(function* () {
		if (observed_canvas === undefined || observed_host === undefined || next_activities.length === 0) return;

		const context = yield* RunBrowserDom(() => observed_canvas.getContext("2d"));
		if (!context) return;

		const { activity_colors, height, pixel_ratio, width } = yield* RunBrowserDom(() => {
			const styles = getComputedStyle(observed_host);
			return {
				activity_colors: [5, 4, 3, 2, 1].map((level) => styles.getPropertyValue(`--chart-${level}`).trim()),
				height: observed_host.clientHeight,
				pixel_ratio: window.devicePixelRatio || 1,
				width: observed_host.clientWidth,
			};
		});
		const activity_by_date = new Map(next_activities.map((activity) => [activity.date, activity]));
		const end = parse_date(next_activities.at(-1)?.date ?? "1970-01-01");
		cell_size = Math.max(MinimumCellSize, Math.min(MaximumCellSize, Math.floor(height / 4) - CellGap));
		const step = cell_size + CellGap;
		column_count = Math.max(1, Math.floor((width + CellGap) / step));
		row_count = Math.max(1, Math.floor((height + CellGap) / step));
		const grid_width = column_count * step - CellGap;
		const grid_height = row_count * step - CellGap;
		grid_left = Math.max(0, Math.floor((width - grid_width) / 2));
		grid_top = Math.max(0, Math.floor((height - grid_height) / 2));
		const max_tokens = Math.max(1, ...next_activities.map((activity) => activity.tokens));
		const next_cells: CalendarCell[] = [];

		yield* RunBrowserDom(() => {
			observed_canvas.width = Math.round(width * pixel_ratio);
			observed_canvas.height = Math.round(height * pixel_ratio);
			observed_canvas.style.width = `${width}px`;
			observed_canvas.style.height = `${height}px`;
			context.scale(pixel_ratio, pixel_ratio);
			context.clearRect(0, 0, width, height);
		});

		for (let row = 0; row < row_count; row += 1) {
			for (let column = 0; column < column_count; column += 1) {
				const offset = row * column_count + column;
				const date = add_days(end, -offset);
				const in_range = offset < next_activities.length;
				const activity = in_range
					? (activity_by_date.get(date_key(date)) ?? { date: date_key(date), engines: [], tokens: 0 })
					: { date: date_key(date), engines: [], tokens: 0 };
				const intensity = activity.tokens === 0 ? 0 : Math.max(1, Math.ceil(Math.sqrt(activity.tokens / max_tokens) * 4));
				const x = grid_left + column * step;
				const y = grid_top + row * step;

				context.fillStyle =
					activity_colors[intensity] ?? activity_colors.at(-1) ?? "transparent";
				context.beginPath();
				context.roundRect(x, y, cell_size, cell_size, Math.min(2, cell_size / 3));
				context.fill();
				if (in_range) next_cells.push({ ...activity, column, row });
			}
		}

		cells = next_cells;
		});

	const InspectCell = (event: PointerEvent & { readonly currentTarget: HTMLCanvasElement }) =>
		Effect.gen(function* () {
		const bounds = yield* RunBrowserDom(() => event.currentTarget.getBoundingClientRect());
		const step = cell_size + CellGap;
		const x = event.clientX - bounds.left - grid_left;
		const y = event.clientY - bounds.top - grid_top;
		const column = Math.floor(x / step);
		const row = Math.floor(y / step);
		const candidate = cells.find((cell) => cell.column === column && cell.row === row);
		const within_cell = x >= 0 && y >= 0 && x % step <= cell_size && y % step <= cell_size;

		hovered = candidate && within_cell ? candidate : undefined;
		tooltip_x = event.clientX - bounds.left;
		tooltip_y = event.clientY - bounds.top;
		});

	const ClearHoveredCell = () =>
		Effect.gen(function* () {
			hovered = undefined;
		});

	/** SER reruns this position when bindings or activities change. */
	yield* Draw(host, canvas, activities);

	const ObserveCalendar = (observed_host: HTMLDivElement, draw_key: string) =>
		Effect.gen(function* () {
			return yield* Effect.acquireRelease(
			Effect.gen(function* () {
				const request_draw = () => calendar_runner.ReplaceUnsafe(draw_key, { _tag: "Draw" });
				const { resize_observer, theme_observer } = yield* RunBrowserDom(() => {
					const resize_observer = new ResizeObserver(request_draw);
					const theme_observer = new MutationObserver(request_draw);
					resize_observer.observe(observed_host);
					theme_observer.observe(document.documentElement, { attributes: true, attributeFilter: ["class", "style"] });
					return { resize_observer, theme_observer };
				});
				request_draw();
				return { resize_observer, theme_observer };
			}),
			({ resize_observer, theme_observer }) =>
				Effect.gen(function* () {
					yield* RunBrowserDom(() => {
						resize_observer.disconnect();
						theme_observer.disconnect();
					});
				}),
			);
		});

	const RunCalendarObservation = (observation: CalendarObservation) =>
		Effect.gen(function* () {
			if (observation._tag === "Draw") {
				yield* Draw(host, canvas, activities);
				return;
			}
			yield* ObserveCalendar(observation.host, observation.draw_key);
			yield* Effect.never;
		});

	const calendar_runner = yield* MakeScopedAttachmentRunner(RunCalendarObservation);

	const observe_calendar = () => {
		return calendar_runner.Attachment({
			_tag: "Observe",
			host,
			draw_key: `calendar-draw:${crypto.randomUUID()}`,
		});
	};
</script>

<div bind:this={host} class="relative min-h-0 w-full flex-1">
	<canvas
		bind:this={canvas}
		use:observe_calendar
		class="block"
		aria-label="Recent daily token activity"
		onpointerleave={yield* ClearHoveredCell()}
		onpointermove={yield* InspectCell(event)}
	></canvas>

	{#if hovered}
		<div
			class="pointer-events-none absolute z-20 w-max min-w-44 -translate-x-1/2 -translate-y-full animate-in fade-in zoom-in-95 overflow-hidden rounded-lg border border-border bg-popover text-xs shadow-lg motion-reduce:animate-none"
			style:left={`${tooltip_x}px`}
			style:top={`${tooltip_y - 8}px`}
		>
			<div class="flex items-baseline justify-between gap-6 px-3 py-2">
				<p class="font-semibold text-foreground">
					{token_label(hovered.tokens)}
					<span class="font-normal text-muted-foreground">tokens</span>
				</p>
				<p class="whitespace-nowrap text-muted-foreground">
					{parse_date(hovered.date).toLocaleDateString("en", { dateStyle: "medium" })}
				</p>
			</div>
			{#if hovered.engines.length > 0}
				<ul class="flex flex-col gap-2 border-t border-border px-3 py-2.5">
					{#each hovered.engines as slice (`${slice.engine_id ?? ""} ${slice.model_id ?? ""}`)}
						{@const presentation = UsageSlicePresentationFor(slice.engine_id, slice.model_id)}
						{@const mark = presentation.mark}
						{@const MarkIcon = mark.icon}
						<li class="flex items-center gap-2">
							<MarkIcon class={EngineMarkClass(mark, "size-3.5")} />
							<span class="font-medium text-foreground">{presentation.label}</span>
							<span class="ml-auto flex items-center gap-1.5 pl-4">
								<svg viewBox="0 0 16 16" class="size-4 -rotate-90" aria-hidden="true">
									<circle cx="8" cy="8" r="6" fill="none" stroke="var(--muted)" stroke-width="3" />
									<circle
										cx="8"
										cy="8"
										r="6"
										fill="none"
										stroke={mark.accent}
										stroke-width="3"
										stroke-linecap="round"
										stroke-dasharray={`${engine_share(hovered, slice) * donut_circumference} ${donut_circumference}`}
									/>
								</svg>
								<span class="tabular-nums text-muted-foreground">{token_label(slice.tokens)}</span>
							</span>
						</li>
					{/each}
				</ul>
			{/if}
		</div>
	{/if}
</div>
