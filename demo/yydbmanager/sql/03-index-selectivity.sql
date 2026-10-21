-- Same shape, different selectivity: customer 1 has 900 orders.
SELECT order_id,ordered_on,status,total_amount FROM orders
WHERE customer_id=1 ORDER BY ordered_on;
SELECT order_id,ordered_on,status,total_amount FROM orders
WHERE customer_id=24 ORDER BY ordered_on;
