.explain off
.headers on
.mode csv
.once sqlite-plan.csv
EXPLAIN QUERY PLAN SELECT order_id, ordered_on FROM orders
WHERE customer_id=1 ORDER BY ordered_on;
