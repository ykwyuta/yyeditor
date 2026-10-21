SELECT pid, usename, state, wait_event_type, wait_event,
       clock_timestamp() - query_start AS query_age,
       clock_timestamp() - xact_start AS transaction_age,
       pg_blocking_pids(pid) AS blocking_pids, query
FROM pg_stat_activity
WHERE datname = current_database() AND pid <> pg_backend_pid()
  AND state <> 'idle'
ORDER BY xact_start NULLS LAST;

SELECT userid, queryid, calls, total_exec_time, mean_exec_time, rows,
       shared_blks_hit, shared_blks_read, temp_blks_written, query
FROM pg_stat_statements
WHERE dbid = (SELECT oid FROM pg_database WHERE datname = current_database())
ORDER BY total_exec_time DESC LIMIT 20;

SELECT datname, xact_commit, xact_rollback, blks_hit, blks_read,
       temp_bytes, deadlocks, stats_reset
FROM pg_stat_database WHERE datname = current_database();
