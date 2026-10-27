PRAGMA enable_profiling='json';
PRAGMA profiling_output='duckdb-plan.json';
SELECT customer_id, COUNT(*) FROM orders GROUP BY customer_id;
PRAGMA disable_profiling;
