\encoding UTF8
\pset format unaligned
\pset tuples_only on
\o postgresql-plan.json
EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)
SELECT customer_id, COUNT(*) FROM orders GROUP BY customer_id;
\o
