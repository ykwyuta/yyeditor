-- Use estimated/actual plan in yydbmanager. This is read-only.
SELECT c.customer_code,c.name AS customer_name,e.name AS sales_rep,
       p.name AS product_name,COUNT(*) AS line_count,
       SUM(i.quantity*i.unit_price) AS revenue
FROM orders o
JOIN customers c ON c.customer_id=o.customer_id
JOIN employees e ON e.employee_id=o.sales_rep_id
JOIN order_items i ON i.order_id=o.order_id
JOIN products p ON p.product_id=i.product_id
WHERE o.status='PAID'
GROUP BY c.customer_code,c.name,e.name,p.name
ORDER BY revenue DESC,c.customer_code,p.name;
