CREATE TABLE IF NOT EXISTS visitors (
  uuid TEXT PRIMARY KEY,
  first_seen TEXT NOT NULL DEFAULT (datetime('now')),
  last_seen TEXT NOT NULL DEFAULT (datetime('now')),
  visit_count INTEGER NOT NULL DEFAULT 1
);
