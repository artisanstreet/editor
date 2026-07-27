<script lang="ts">
	import { onMount } from "svelte";

export type CalendarActivity = {
		date: string;
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
	let host: HTMLDivElement;
	let canvas: HTMLCanvasElement;
	let hovered = $state<CalendarCell | undefined>();
	let tooltip_x = $state(0);
	let tooltip_y = $state(0);
	let cells: ReadonlyArray<CalendarCell> = [];
	let cell_size = MaximumCellSize;
	let grid_left = 0;
	let grid_top = 0;
	let column_count = 0;
	let row_count = 0;

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

	const draw = () => {
		if (!canvas || !host || activities.length === 0) return;

		const context = canvas.getContext("2d");
		if (!context) return;

		const styles = getComputedStyle(host);
		const activity_colors = [5, 4, 3, 2, 1].map((level) => styles.getPropertyValue(`--chart-${level}`).trim());
		const activity_by_date = new Map(activities.map((activity) => [activity.date, activity]));
		const end = parse_date(activities.at(-1)?.date ?? "1970-01-01");
		const width = host.clientWidth;
		const height = host.clientHeight;
		cell_size = Math.max(MinimumCellSize, Math.min(MaximumCellSize, Math.floor(height / 4) - CellGap));
		const step = cell_size + CellGap;
		column_count = Math.max(1, Math.floor((width + CellGap) / step));
		row_count = Math.max(1, Math.floor((height + CellGap) / step));
		const grid_width = column_count * step - CellGap;
		const grid_height = row_count * step - CellGap;
		grid_left = Math.max(0, Math.floor((width - grid_width) / 2));
		grid_top = Math.max(0, Math.floor((height - grid_height) / 2));
		const pixel_ratio = window.devicePixelRatio || 1;
		const max_tokens = Math.max(1, ...activities.map((activity) => activity.tokens));
		const next_cells: CalendarCell[] = [];

		canvas.width = Math.round(width * pixel_ratio);
		canvas.height = Math.round(height * pixel_ratio);
		canvas.style.width = `${width}px`;
		canvas.style.height = `${height}px`;
		context.scale(pixel_ratio, pixel_ratio);
		context.clearRect(0, 0, width, height);

		for (let row = 0; row < row_count; row += 1) {
			for (let column = 0; column < column_count; column += 1) {
				const offset = row * column_count + column;
				const date = add_days(end, -offset);
				const in_range = offset < activities.length;
				const activity = in_range ? (activity_by_date.get(date_key(date)) ?? { date: date_key(date), tokens: 0 }) : { date: date_key(date), tokens: 0 };
				const intensity = activity.tokens === 0 ? 0 : Math.max(1, Math.ceil(Math.sqrt(activity.tokens / max_tokens) * 4));
				const x = grid_left + column * step;
				const y = grid_top + row * step;

				context.fillStyle = activity_colors[intensity] ?? activity_colors.at(-1)!;
				context.beginPath();
				context.roundRect(x, y, cell_size, cell_size, Math.min(2, cell_size / 3));
				context.fill();
				if (in_range) next_cells.push({ ...activity, column, row });
			}
		}

		cells = next_cells;
	};

	const inspect_cell = (event: PointerEvent) => {
		const bounds = canvas.getBoundingClientRect();
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
	};

	/** Redraws when activities arrive asynchronously, not only on mount and resize. */
	$effect(draw);

	onMount(() => {
		const resize_observer = new ResizeObserver(draw);
		const theme_observer = new MutationObserver(draw);
		resize_observer.observe(host);
		theme_observer.observe(document.documentElement, { attributes: true, attributeFilter: ["class", "style"] });
		draw();

		return () => {
			resize_observer.disconnect();
			theme_observer.disconnect();
		};
	});
</script>

<div bind:this={host} class="relative min-h-0 w-full flex-1">
	<canvas
		bind:this={canvas}
		class="block"
		aria-label="Recent daily token activity"
		onpointerleave={() => (hovered = undefined)}
		onpointermove={inspect_cell}
	></canvas>

	{#if hovered}
		<div
			class="pointer-events-none absolute z-20 w-max min-w-28 -translate-x-1/2 -translate-y-full rounded-md border border-border bg-popover px-2.5 py-2 text-xs shadow-md"
			style:left={`${tooltip_x}px`}
			style:top={`${tooltip_y - 8}px`}
		>
			<p class="font-medium text-foreground">{token_label(hovered.tokens)} tokens</p>
			<p class="whitespace-nowrap text-muted-foreground">{parse_date(hovered.date).toLocaleDateString("en", { dateStyle: "medium" })}</p>
		</div>
	{/if}
</div>
