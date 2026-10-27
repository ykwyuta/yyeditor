-- Export as CSV/Parquet and check the text: integers exceed JavaScript's 2^53.
SELECT * FROM exact_values ORDER BY value_id;
-- These columns have no physical FK. Add/recommend a logical ER relationship.
SELECT t.ticket_id,t.customer_code,c.name,t.subject,t.priority
FROM support_tickets t JOIN customers c ON c.customer_code=t.customer_code
ORDER BY t.ticket_id;
