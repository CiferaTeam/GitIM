// Single source of truth for kind → visual style mapping. Previously this
// lived in both `cron-calendar.tsx` (chips on day cells) and
// `cron-day-panel.tsx` (badges in the day list), with subtly different
// opacities (`/15` vs `/25` vs `/30`) — easy to drift out of sync as the
// design system changes.
//
// Opacity convention: `bg-X/15`, `border-X/30`, `text-X`. This matches
// `agent-status.tsx`, the closest existing precedent in the codebase
// (badge with semantic color). `index.css` exposes the X tokens, so any
// future palette change propagates here automatically.
//
// `Icon` is the WCAG 1.4.1 non-color affordance — colour-blind users and
// screen-reader navigators see a glyph plus the textual `label`, not just
// a hue.

import { AlertCircle, Check, Clock, type LucideIcon } from "lucide-react";
import type { CronTimelineKind } from "@/lib/types";

export interface KindStyle {
  /** Solid background for the small dot in calendar chips. */
  dot: string;
  /** Tailwind classes for the chip / badge background + text + border. */
  chip: string;
  /** Chinese-language kind label as it appears to the user. */
  label: string;
  /** Visible icon — non-color affordance per WCAG 1.4.1. */
  Icon: LucideIcon;
}

const STYLES: Record<CronTimelineKind, KindStyle> = {
  past: {
    dot: "bg-success",
    chip: "bg-success/15 text-success border-success/30",
    label: "已执行",
    Icon: Check,
  },
  future: {
    dot: "bg-primary",
    chip: "bg-primary/15 text-primary border-primary/30",
    label: "未来",
    Icon: Clock,
  },
  missed: {
    dot: "bg-error",
    chip: "bg-error/15 text-error border-error/30",
    label: "未执行",
    Icon: AlertCircle,
  },
};

/** Lookup with a defensive fallback. Catalogue churn at the runtime — e.g.
 *  a future "failed" kind landing without a frontend rev — would otherwise
 *  crash the calendar with `Cannot read property 'chip' of undefined`. The
 *  `missed` style is the safest "I don't know what to draw" choice: it's
 *  the most visually attention-getting (error red), which is exactly what
 *  an unknown-status entry should look like. */
export function kindStyle(kind: CronTimelineKind | string): KindStyle {
  return STYLES[kind as CronTimelineKind] ?? STYLES.missed;
}

export { STYLES as KIND_STYLES };
