export const schema = `
CREATE TABLE IF NOT EXISTS automation_creations (
  id TEXT PRIMARY KEY, definition TEXT
);
CREATE TABLE IF NOT EXISTS automations (
  id TEXT PRIMARY KEY, thread_id TEXT NOT NULL UNIQUE, definition TEXT NOT NULL,
  revision INTEGER NOT NULL, state TEXT NOT NULL CHECK(state IN ('active','paused')),
  pause_reason TEXT, cursor INTEGER NOT NULL, missed_count INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS automation_placements (
  id TEXT PRIMARY KEY REFERENCES automation_creations(id), cursor INTEGER NOT NULL,
  submitted INTEGER NOT NULL DEFAULT 0, error TEXT
);
CREATE TABLE IF NOT EXISTS automation_clicks (
  automation_id TEXT NOT NULL REFERENCES automations(id) ON DELETE CASCADE,
  click TEXT NOT NULL, request_id TEXT NOT NULL, due INTEGER NOT NULL,
  PRIMARY KEY(automation_id, click)
);
CREATE TABLE IF NOT EXISTS automation_occurrences (
  automation_id TEXT NOT NULL REFERENCES automations(id) ON DELETE CASCADE,
  request_id TEXT NOT NULL, revision INTEGER NOT NULL, due INTEGER NOT NULL,
  outcome TEXT NOT NULL, detail TEXT, PRIMARY KEY(automation_id, request_id)
);
CREATE TABLE IF NOT EXISTS automation_claims (
  id TEXT PRIMARY KEY, automation_id TEXT NOT NULL REFERENCES automations(id) ON DELETE CASCADE,
  occurrence INTEGER, due INTEGER NOT NULL, revision INTEGER NOT NULL,
  UNIQUE(automation_id, occurrence)
);
INSERT OR IGNORE INTO automation_claims
  SELECT request_id, automation_id, CASE WHEN request_id = automation_id || '-' || due THEN due END, due, revision
  FROM automation_occurrences;
INSERT OR IGNORE INTO automation_claims
  SELECT c.request_id, c.automation_id, CASE WHEN c.request_id = c.automation_id || '-' || c.due THEN c.due END, c.due, a.revision
  FROM automation_clicks c JOIN automations a ON a.id=c.automation_id;
`;
