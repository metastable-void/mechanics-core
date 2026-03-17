# mechanics-core audit findings (2026-03-17)

All previously tracked findings are now resolved.

## Status
- `1` Pending Promise result handling: done.
- `2` Unhandled async error propagation: done.
- `3` `try_run` naming mismatch: done (`run_try_enqueue`).
- `4` Heuristic reply-timeout model: done (`run_timeout` + `RunTimeout`).
- `5` HTTP non-success status validation: done (strict by default, explicit opt-in).
- `6` Invalid configured header handling: done (strict validation, explicit error).
- `7` Runtime panic/unwrap paths: done (converted to structured failures).
- `8` Worker startup lock scope: done (rendezvous send moved outside map lock scope).
- `9` `parking_lot` unused dependency: done (pool synchronization migrated).
- `10` Tokio feature breadth: done (`default-features = false`, minimal features).

## Current open items
- None identified in the maintained audit list.
