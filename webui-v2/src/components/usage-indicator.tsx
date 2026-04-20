import { useStats } from "@/hooks/use-stats";
import {
  HoverCard,
  HoverCardContent,
  HoverCardTrigger,
} from "@/components/ui/hover-card";

// Builds an SVG `<path d="">` string for a polyline sparkline.
// Exported for isolated testing.
export function sparklinePath(values: number[], w: number, h: number): string {
  if (values.length === 0) return "";
  if (values.length === 1) {
    const y = h / 2;
    return `M0,${y.toFixed(1)} L${w.toFixed(1)},${y.toFixed(1)}`;
  }
  // max(1, ...) avoids div-by-zero when every day is still 0 — we just draw
  // a flat line along the bottom instead of NaN coords.
  const max = Math.max(1, ...values);
  const step = w / (values.length - 1);
  return values
    .map((v, i) => {
      const x = i * step;
      const y = h - (v / max) * h;
      return `${i === 0 ? "M" : "L"}${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(" ");
}

interface SparklineProps {
  values: number[];
  width: number;
  height: number;
  strokeWidth?: number;
}

function Sparkline({
  values,
  width,
  height,
  strokeWidth = 1.5,
}: SparklineProps) {
  return (
    <svg
      width={width}
      height={height}
      viewBox={`0 0 ${width} ${height}`}
      aria-hidden="true"
      className="overflow-visible"
    >
      <path
        d={sparklinePath(values, width, height)}
        fill="none"
        stroke="currentColor"
        strokeWidth={strokeWidth}
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

/**
 * Community pulse: today's DAU + 30-day sparkline in the top bar, with a
 * larger chart revealed on hover. Data pulled once on mount from the public
 * `/api/stats` endpoint; if that fetch fails the whole indicator hides
 * silently rather than showing a broken state.
 */
export function UsageIndicator() {
  const days = useStats();
  if (!days || days.length === 0) return null;

  const values = days.map((d) => d.dau);
  const today = values[values.length - 1] ?? 0;
  const peak = Math.max(0, ...values);

  return (
    <HoverCard>
      <HoverCardTrigger asChild>
        <button
          type="button"
          aria-label={`今日 ${today} 人在用 · 30 天趋势`}
          className="flex items-center gap-1.5 h-7 px-2 rounded-md text-primary hover:bg-surface/60 transition-colors"
        >
          <Sparkline values={values} width={36} height={14} />
          <span className="text-xs font-mono">{today}</span>
        </button>
      </HoverCardTrigger>
      <HoverCardContent className="w-64">
        <div className="space-y-2">
          <div className="text-sm font-medium text-foreground">
            {today} 人正在使用 GitIM·Cell
          </div>
          <div className="text-primary">
            <Sparkline values={values} width={224} height={56} strokeWidth={2} />
          </div>
          <div className="text-xs text-text-muted">
            近 30 天 · 峰值 {peak}
          </div>
        </div>
      </HoverCardContent>
    </HoverCard>
  );
}
