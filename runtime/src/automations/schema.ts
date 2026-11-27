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
CREATE TABLE IF NOT EXISTS automation_folder_checks (
  id TEXT PRIMARY KEY REFERENCES automation_placements(id) ON DELETE CASCADE
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
CREATE TABLE IF NOT EXISTS automation_claim_windows (
  id TEXT PRIMARY KEY REFERENCES automation_claims(id) ON DELETE CASCADE, catch_up_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS automation_unknown_windows (
  id TEXT PRIMARY KEY REFERENCES automation_claims(id) ON DELETE CASCADE
);
INSERT OR IGNORE INTO automation_claims(id,automation_id,occurrence,due,revision)
  SELECT request_id, automation_id, CASE WHEN request_id = automation_id || '-' || due THEN due END, due, revision
  FROM automation_occurrences;
INSERT OR IGNORE INTO automation_claims(id,automation_id,occurrence,due,revision)
  SELECT c.request_id, c.automation_id, CASE WHEN c.request_id = c.automation_id || '-' || c.due THEN c.due END, c.due, a.revision
  FROM automation_clicks c JOIN automations a ON a.id=c.automation_id;
-- A downgraded scheduler may use the edited window and allocate a manual ID.
-- Give that ID a definitive receipt before its launch, in the click transaction.
CREATE TRIGGER IF NOT EXISTS automation_rollback_click AFTER INSERT ON automation_clicks
WHEN NEW.request_id GLOB 'manual-*'
BEGIN
  UPDATE automation_clicks SET request_id=scheduled.id,due=scheduled.due
    FROM (SELECT c.id,c.due FROM automation_claims c
      LEFT JOIN automation_claim_windows w ON w.id=c.id
      LEFT JOIN automation_unknown_windows u ON u.id=c.id
      WHERE c.automation_id=NEW.automation_id AND c.occurrence<=NEW.due
        AND (u.id IS NOT NULL OR NEW.due-c.occurrence<=w.catch_up_ms)
      ORDER BY c.occurrence DESC LIMIT 1) AS scheduled
    WHERE automation_id=NEW.automation_id AND click=NEW.click;
  INSERT OR IGNORE INTO automation_occurrences(automation_id,request_id,revision,due,outcome,detail)
    SELECT NEW.automation_id, NEW.request_id, m.revision, NEW.due, 'skipped',
      'Scheduled occurrence already claimed: ' || click.request_id
    FROM automation_clicks click JOIN automation_claims m ON m.id=NEW.request_id
    WHERE click.automation_id=NEW.automation_id AND click.click=NEW.click AND click.request_id<>NEW.request_id;
END;
`;
