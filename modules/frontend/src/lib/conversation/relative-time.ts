export const format_relative_age = (now: number, timestamp: string): string => {
	const timestamp_ms = Date.parse(timestamp);
	const elapsed_seconds = Number.isFinite(timestamp_ms)
		? Math.max(0, Math.floor((now - timestamp_ms) / 1_000))
		: 0;

	if (elapsed_seconds < 60) return `${elapsed_seconds}s ago`;

	const elapsed_minutes = Math.floor(elapsed_seconds / 60);
	if (elapsed_minutes < 60) return `${elapsed_minutes}m ago`;

	const elapsed_hours = Math.floor(elapsed_minutes / 60);
	if (elapsed_hours < 24) return `${elapsed_hours}h ago`;

	const elapsed_days = Math.floor(elapsed_hours / 24);
	if (elapsed_days < 7) return `${elapsed_days}d ago`;

	const elapsed_weeks = Math.floor(elapsed_days / 7);
	if (elapsed_weeks < 5) return `${elapsed_weeks}w ago`;

	const elapsed_months = Math.floor(elapsed_days / 30);
	if (elapsed_months < 12) return `${elapsed_months}mo ago`;

	return `${Math.floor(elapsed_days / 365)}y ago`;
};
