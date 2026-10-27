SELECT TOP (20) r.session_id, r.status, r.blocking_session_id,
       r.wait_type, r.wait_time AS wait_ms, r.cpu_time AS cpu_ms,
       r.total_elapsed_time AS elapsed_ms, r.logical_reads,
       t.text AS batch_text
FROM sys.dm_exec_requests AS r
OUTER APPLY sys.dm_exec_sql_text(r.sql_handle) AS t
WHERE r.session_id <> @@SPID
ORDER BY r.total_elapsed_time DESC;

SELECT TOP (20) qs.sql_handle, qs.statement_start_offset,
       qs.execution_count, qs.total_worker_time / 1000.0 AS cpu_ms,
       qs.total_elapsed_time / 1000.0 AS elapsed_ms,
       qs.total_elapsed_time / 1000.0 / NULLIF(qs.execution_count, 0) AS average_ms,
       qs.total_logical_reads, qs.total_physical_reads, t.text AS batch_text
FROM sys.dm_exec_query_stats AS qs
CROSS APPLY sys.dm_exec_sql_text(qs.sql_handle) AS t
ORDER BY qs.total_worker_time DESC;

SELECT TOP (20) wait_type, waiting_tasks_count, wait_time_ms, signal_wait_time_ms
FROM sys.dm_os_wait_stats
WHERE waiting_tasks_count > 0 ORDER BY wait_time_ms DESC;
