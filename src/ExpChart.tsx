import { useEffect, useRef } from "react";
import uPlot from "uplot";
import "uplot/dist/uPlot.min.css";

export type ChartPoint = { t: number; total: number | null };

type Props = {
  samples: ChartPoint[];
  height?: number;
};

function compact(value: number): string {
  const abs = Math.abs(value);
  if (abs >= 1_000_000_000) return `${(value / 1_000_000_000).toFixed(1)}B`;
  if (abs >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
  if (abs >= 1_000) return `${Math.round(value / 1_000)}k`;
  return String(Math.round(value));
}

// The chart shows cumulative EXP gained over the plotted span, so the first
// sample is the baseline. Plotting lifetime totals collapses the axis when a
// session only moves a few thousand inside a ~1.1M total.
function chartData(samples: ChartPoint[]): uPlot.AlignedData {
  const base = samples.find((sample) => sample.total !== null)?.total ?? 0;
  return [
    samples.map((sample) => sample.t),
    samples.map((sample) => (sample.total === null ? null : sample.total - base)),
  ];
}

export default function ExpChart({ samples, height = 150 }: Props) {
  const host = useRef<HTMLDivElement>(null);
  const chart = useRef<uPlot | null>(null);

  useEffect(() => {
    if (!host.current) return;

    const options: uPlot.Options = {
      width: host.current.clientWidth,
      height,
      padding: [8, 10, 0, 0],
      cursor: { drag: { x: true, y: false } },
      legend: { show: false },
      scales: { x: { time: true }, y: { auto: true } },
      axes: [
        {
          stroke: "#64748b",
          grid: { stroke: "rgba(100,116,139,0.15)" },
          ticks: { stroke: "rgba(100,116,139,0.25)" },
          font: "10px Segoe UI",
        },
        {
          stroke: "#64748b",
          grid: { stroke: "rgba(100,116,139,0.15)" },
          ticks: { stroke: "rgba(100,116,139,0.25)" },
          font: "10px Segoe UI",
          size: 52,
          values: (_u, splits) => splits.map((v) => compact(v)),
        },
      ],
      series: [
        {},
        {
          stroke: "#4ade80",
          width: 2,
          fill: "rgba(74,222,128,0.08)",
          paths: uPlot.paths.stepped!({ align: 1 }),
        },
      ],
    };

    const data = chartData(samples);

    chart.current = new uPlot(options, data, host.current);

    const resize = () => {
      if (host.current && chart.current) {
        chart.current.setSize({ width: host.current.clientWidth, height });
      }
    };
    window.addEventListener("resize", resize);

    return () => {
      window.removeEventListener("resize", resize);
      chart.current?.destroy();
      chart.current = null;
    };
  }, [height]);

  useEffect(() => {
    chart.current?.setData(chartData(samples));
  }, [samples]);

  return <div className="chart" ref={host} />;
}
