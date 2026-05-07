CREATE TABLE IF NOT EXISTS visits (
  uuid TEXT NOT NULL,
  day  TEXT NOT NULL,
  PRIMARY KEY (uuid, day)
);

CREATE INDEX IF NOT EXISTS idx_visits_day ON visits(day);
