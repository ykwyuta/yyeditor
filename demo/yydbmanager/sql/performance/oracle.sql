SELECT sid, serial#, username, sql_id, event, wait_class, state,
       blocking_session
FROM v$session
WHERE type = 'USER' AND username IS NOT NULL AND status = 'ACTIVE';

SELECT sql_id, child_number, plan_hash_value, executions,
       cpu_time / 1000000 AS cpu_seconds,
       elapsed_time / 1000000 AS elapsed_seconds,
       elapsed_time / 1000000 / NULLIF(executions, 0) AS average_seconds,
       buffer_gets, disk_reads, rows_processed, sql_text
FROM v$sql
WHERE parsing_schema_name = USER
ORDER BY elapsed_time DESC FETCH FIRST 20 ROWS ONLY;
