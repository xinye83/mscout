// The overlay's graph: the active EXP rate over the last hour, with the
// session's average as a dashed line and deaths, level-ups and map changes
// marked. A rate shows a slowdown as a dip; the cumulative curve it replaces
// only ever rose, so the rate was its slope and idle a flat stretch.

import { useEffect, useRef } from "react";
import uPlot from "uplot";
import "uplot/dist/uPlot.min.css";

export type RatePoint = { t: number; rate: number | null };
export type Marker = { t: number; kind: "death" | "level" | "map" };

type Props = {
  points: RatePoint[];
  average: number | null;
  markers: Marker[];
  height?: number;
};

const MARKER_COLOURS: Record<Marker["kind"], string> = {
  death: "#f87171",
  level: "#facc15",
  map: "#60a5fa",
};

function compact(value: number): string {
  const abs = Math.abs(value);
  if (abs >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
  if (abs >= 1_000) return `${Math.round(value / 1_000)}k`;
  return String(Math.round(value));
}

function data(points: RatePoint[], average: number | null): uPlot.AlignedData {
  return [
    points.map((point) => point.t),
    points.map((point) => point.rate),
    points.map(() => average),
  ];
}

export default function RateChart({ points, average, markers, height = 110 }: Props) {
  const host = useRef<HTMLDivElement>(null);
  const chart = useRef<uPlot | null>(null);
  const markerRef = useRef<Marker[]>(markers);
  markerRef.current = markers;

  useEffect(() => {
    if (!host.current) return;
    const axis = {
      stroke: "#64748b",
      grid: { stroke: "rgba(100,116,139,0.15)" },
      ticks: { stroke: "rgba(100,116,139,0.25)" },
      font: "10px Segoe UI",
    };
    const options: uPlot.Options = {
      width: host.current.clientWidth,
      height,
      padding: [8, 10, 0, 0],
      cursor: { show: false },
      legend: { show: false },
      scales: { x: { time: true }, y: { auto: true, range: (_u, _min, max) => [0, Math.max(1, max) * 1.1] } },
      axes: [axis, { ...axis, size: 44, values: (_u, splits) => splits.map((v) => compact(v)) }],
      series: [
        {},
        { stroke: "#4ade80", width: 2, fill: "rgba(74,222,128,0.08)", spanGaps: false },
        { stroke: "rgba(148,163,184,0.7)", width: 1, dash: [4, 4] },
      ],
      hooks: {
        draw: [
          (u) => {
            const ctx = u.ctx;
            const [x0, x1] = [u.scales.x.min ?? 0, u.scales.x.max ?? 0];
            ctx.save();
            ctx.lineWidth = 1 * devicePixelRatio;
            for (const marker of markerRef.current) {
              if (marker.t < x0 || marker.t > x1) continue;
              const x = Math.round(u.valToPos(marker.t, "x", true));
              ctx.strokeStyle = MARKER_COLOURS[marker.kind];
              ctx.beginPath();
              ctx.moveTo(x, u.bbox.top);
              ctx.lineTo(x, u.bbox.top + u.bbox.height);
              ctx.stroke();
            }
            ctx.restore();
          },
        ],
      },
    };
    chart.current = new uPlot(options, data(points, average), host.current);
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
    // The chart is built once per height; data changes go through setData.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [height]);

  useEffect(() => {
    chart.current?.setData(data(points, average));
  }, [points, average, markers]);

  return <div className="chart" ref={host} />;
}
