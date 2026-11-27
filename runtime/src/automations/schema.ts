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
CREATE TABLE IF NOT EXISTS automation_uncertain_clicks (
  automation_id TEXT NOT NULL, click TEXT NOT NULL, PRIMARY KEY(automation_id,click),
  FOREIGN KEY(automation_id,click) REFERENCES automation_clicks(automation_id,click) ON DELETE CASCADE
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
CREATE TABLE IF NOT EXISTS automation_click_ranges (
  automation_id TEXT NOT NULL REFERENCES automations(id) ON DELETE CASCADE,
  start INTEGER NOT NULL, request_id TEXT, PRIMARY KEY(automation_id,start)
) WITHOUT ROWID;
-- A downgraded scheduler may use the edited window and allocate a manual ID.
-- Give that ID a definitive receipt before its launch, in the click transaction.
DROP TRIGGER IF EXISTS automation_rollback_click;
CREATE TRIGGER automation_rollback_click AFTER INSERT ON automation_clicks
WHEN NEW.request_id GLOB 'manual-*'
BEGIN
  UPDATE automation_clicks SET request_id=scheduled.id,due=scheduled.due
    FROM automation_claims AS scheduled
    WHERE scheduled.id=(SELECT request_id FROM automation_click_ranges
      WHERE automation_id=NEW.automation_id AND start<=NEW.due ORDER BY start DESC LIMIT 1)
      AND automation_clicks.automation_id=NEW.automation_id AND click=NEW.click;
  INSERT OR IGNORE INTO automation_occurrences(automation_id,request_id,revision,due,outcome,detail)
    SELECT NEW.automation_id, NEW.request_id, m.revision, NEW.due, 'skipped',
      'Scheduled occurrence already claimed: ' || click.request_id
    FROM automation_clicks click JOIN automation_claims m ON m.id=NEW.request_id
    WHERE click.automation_id=NEW.automation_id AND click.click=NEW.click AND click.request_id<>NEW.request_id;
END;
-- The previous timer scans every unreceipted claim after inserting the new hour.
DROP TRIGGER IF EXISTS automation_supersede_claim;
CREATE TRIGGER automation_supersede_claim AFTER INSERT ON automation_claims
WHEN NEW.occurrence IS NOT NULL
BEGIN
  -- A negative revision prevents submission while allowing the old timer to
  -- reconcile host acceptance before writing its stale-revision receipt.
  UPDATE automation_claims SET revision=-revision
    WHERE automation_id=NEW.automation_id AND occurrence<NEW.occurrence AND revision>0
      AND id IN (SELECT id FROM automation_claim_windows)
      AND NOT EXISTS (SELECT 1 FROM automation_occurrences WHERE automation_id=NEW.automation_id AND request_id=automation_claims.id);
END;
CREATE TRIGGER IF NOT EXISTS automation_superseded_receipt AFTER INSERT ON automation_occurrences
WHEN (SELECT revision FROM automation_claims WHERE id=NEW.request_id)<0
BEGIN
  UPDATE automation_occurrences SET revision=abs(NEW.revision),
    outcome=CASE WHEN NEW.outcome='skipped' THEN 'missed' ELSE NEW.outcome END,
    detail=CASE WHEN NEW.outcome='skipped' THEN NULL ELSE NEW.detail END
    WHERE automation_id=NEW.automation_id AND request_id=NEW.request_id;
  UPDATE automation_claims SET revision=-revision WHERE id=NEW.request_id;
  UPDATE automations SET missed_count=missed_count+1 WHERE id=NEW.automation_id AND NEW.outcome='skipped';
END;
`;
