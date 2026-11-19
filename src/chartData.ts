import type { ChartPoint } from "./ExpChart";

/**
 * Reduce the point count for the chart, keeping the gaps.
 *
 * A `null` point is a break in the line (a period with no recording), not a data
 * point, so it must survive thinning. Keeping only each bucket's last point — the
 * obvious version — drops a gap marker whenever a real sample follows it in the
 * same bucket, which silently joins the line across an outage. That only happens
 * once there are enough rows to thin, which is why it showed up on large ranges.
 */
export function downsample(points: ChartPoint[], max: number): ChartPoint[] {
  if (points.length <= max) return points;
  const bucket = Math.ceil(points.length / max);
  const out: ChartPoint[] = [];

  for (let i = 0; i < points.length; i += bucket) {
    const slice = points.slice(i, i + bucket);

    // Every break in this bucket, then the bucket's last point. A break is never
    // dropped, and times stay ascending because the break precedes the samples it
    // separates.
    for (const point of slice) {
      if (point.total === null) out.push(point);
    }

    const last = slice[slice.length - 1];
    if (last && last.total !== null) out.push(last);
  }

  return out;
}
