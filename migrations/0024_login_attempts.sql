-- Fixed-window login attempt counters, keyed by "email:<addr>" or "ip:<addr>".
-- A row whose window has elapsed is reset on the next attempt, so counters
-- expire on their own.
CREATE TABLE login_attempts (
  key TEXT PRIMARY KEY,
  window_start TIMESTAMPTZ NOT NULL DEFAULT now(),
  attempts INT NOT NULL DEFAULT 0
);
