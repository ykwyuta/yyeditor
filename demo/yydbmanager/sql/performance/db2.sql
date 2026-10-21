SELECT member, application_handle, total_cpu_time, total_act_time, total_act_wait_time
FROM TABLE(SYSPROC.MON_GET_CONNECTION(NULL, -2)) AS m
ORDER BY total_act_wait_time DESC FETCH FIRST 20 ROWS ONLY;

SELECT member, HEX(executable_id) AS executable_id, num_executions,
       num_coord_exec_with_metrics, total_act_time, total_act_wait_time,
       total_cpu_time, rows_read, rows_returned, VARCHAR(stmt_text, 1000) AS stmt_text
FROM TABLE(SYSPROC.MON_GET_PKG_CACHE_STMT(NULL, NULL, NULL, -2)) AS m
ORDER BY total_act_time DESC FETCH FIRST 20 ROWS ONLY;

SELECT req_application_handle, hld_application_handle, req_stmt_text
FROM SYSIBMADM.MON_LOCKWAITS FETCH FIRST 20 ROWS ONLY;
