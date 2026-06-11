import type { CardFilterState } from "./card-filter-bar";

// Pure URL ↔ filter mapping, kept out of card-kanban.tsx so that file only
// exports components (react-refresh/only-export-components).

export function readFilterFromURL(params: URLSearchParams): CardFilterState {
  const assignee = params.get("assignee");
  return {
    channels: params.getAll("channel"),
    labels: params.getAll("label"),
    // Canonical "my cards" form is assignee=__me__; both the toggle and the
    // URL bind to the same field to keep a single source of truth.
    assignee: assignee === "__me__" ? null : assignee,
    mineOnly: assignee === "__me__",
    project: params.get("project"),
  };
}

export function writeFilterToURL(filter: CardFilterState): URLSearchParams {
  const p = new URLSearchParams();
  for (const ch of filter.channels) p.append("channel", ch);
  for (const l of filter.labels) p.append("label", l);
  if (filter.mineOnly) {
    p.set("assignee", "__me__");
  } else if (filter.assignee) {
    p.set("assignee", filter.assignee);
  }
  if (filter.project) {
    p.set("project", filter.project);
  }
  return p;
}
