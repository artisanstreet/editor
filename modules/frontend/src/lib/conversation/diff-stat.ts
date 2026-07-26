/** Fits a diff count into T3 Code's fixed four-character statistic column. */
export const format_compact_diff_count = (value: number): string => {
	if (value < 1_000) return String(value);
	if (value < 1_000_000) {
		const thousands = value / 1_000;
		return `${thousands < 10 ? thousands.toFixed(1).replace(/\.0$/, "") : Math.round(thousands)}k`;
	}
	if (value < 1_000_000_000) {
		const millions = value / 1_000_000;
		return `${millions < 10 ? millions.toFixed(1).replace(/\.0$/, "") : Math.round(millions)}M`;
	}
	const billions = value / 1_000_000_000;
	return `${billions < 10 ? billions.toFixed(1).replace(/\.0$/, "") : Math.round(billions)}B`;
};
