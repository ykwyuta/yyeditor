SELECT tag, memory_usage_bytes, temporary_storage_bytes
FROM duckdb_memory() ORDER BY memory_usage_bytes DESC;

SELECT path, size FROM duckdb_temporary_files();

SELECT name, value FROM duckdb_settings()
WHERE name IN ('threads', 'memory_limit', 'temp_directory');

EXPLAIN ANALYZE SELECT customer_id, COUNT(*) AS order_count
FROM orders GROUP BY customer_id;
