SELECT 'departments' AS table_name, 3 AS expected_rows, COUNT(*) AS actual_rows FROM departments
UNION ALL
SELECT 'employees' AS table_name, 8 AS expected_rows, COUNT(*) AS actual_rows FROM employees
UNION ALL
SELECT 'customers' AS table_name, 24 AS expected_rows, COUNT(*) AS actual_rows FROM customers
UNION ALL
SELECT 'products' AS table_name, 8 AS expected_rows, COUNT(*) AS actual_rows FROM products
UNION ALL
SELECT 'orders' AS table_name, 1200 AS expected_rows, COUNT(*) AS actual_rows FROM orders
UNION ALL
SELECT 'order_items' AS table_name, 3600 AS expected_rows, COUNT(*) AS actual_rows FROM order_items
UNION ALL
SELECT 'support_tickets' AS table_name, 6 AS expected_rows, COUNT(*) AS actual_rows FROM support_tickets
UNION ALL
SELECT 'exact_values' AS table_name, 3 AS expected_rows, COUNT(*) AS actual_rows FROM exact_values
UNION ALL
SELECT 'demo_ready' AS table_name, 1 AS expected_rows, COUNT(*) AS actual_rows FROM demo_ready
ORDER BY table_name;
