# --json output

- Every `--json` document is one JSON object on stdout with `schema_version` (now 1)
  and `decisions` (the decision points to hand back; empty for most commands). Errors
  are `{"schema_version": 1, "error": {"code", "message", ...}}`.
- Fields are only added. An existing field does not change its meaning, type or name and
  is not removed, so read the fields you know and ignore the others.
- `kr run` / `kr gate` / `kr decide`: `ok` (every item passed), `concluded` (false while
  decision points wait), `passed`, `total`, `items` (each with `passed`, `tests` counts
  per outcome, `flaky`, `summary`, `xcresult`, `cases`, and `decisions`: the IDs of its
  decision points), `build`, `duration_ms`, `aborted`.
- Exit codes: 0 passed, 1 failed, 2 bad arguments, 4 decision points pending (only
  check, run, gate and decide), 3 only `kr plan --exit-code` with work to do.
