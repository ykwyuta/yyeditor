EXPLAIN QUERY PLAN SELECT order_id, ordered_on FROM orders
WHERE customer_id = 1 ORDER BY ordered_on;

PRAGMA page_count;
PRAGMA page_size;
PRAGMA freelist_count;
PRAGMA data_version;
PRAGMA journal_mode;
