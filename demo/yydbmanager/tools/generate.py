#!/usr/bin/env python3
"""Regenerate checked-in, deterministic SQL/text fixtures; not needed at startup."""
from pathlib import Path
from decimal import Decimal
import csv
import json
import datetime

ROOT = Path(__file__).resolve().parents[1]
KINDS = ['postgres', 'mysql', 'sqlserver', 'oracle', 'db2', 'sqlite', 'duckdb']
EXPECTED = {'departments': 3, 'employees': 8, 'customers': 24, 'products': 8,
            'orders': 1200, 'order_items': 3600, 'support_tickets': 6, 'exact_values': 3, 'demo_ready': 1}


def schema(kind):
    varchar = lambda n: f'{"NVARCHAR" if kind == "sqlserver" else "VARCHAR2" if kind == "oracle" else "VARCHAR"}({n})'
    money = 'DECIMAL(18,2)'
    exact = 'TEXT' if kind == 'sqlite' else 'DECIMAL(31,8)'
    tables = [
        ('departments', f'department_id INTEGER NOT NULL PRIMARY KEY, name {varchar(100)} NOT NULL'),
        ('employees', f'employee_id INTEGER NOT NULL PRIMARY KEY, department_id INTEGER NOT NULL, manager_id INTEGER, name {varchar(100)} NOT NULL, email {varchar(160)} NOT NULL UNIQUE, salary {money} NOT NULL, notes {varchar(400)}, CONSTRAINT fk_employee_department FOREIGN KEY(department_id) REFERENCES departments(department_id), CONSTRAINT fk_employee_manager FOREIGN KEY(manager_id) REFERENCES employees(employee_id)'),
        ('customers', f'customer_id INTEGER NOT NULL PRIMARY KEY, customer_code {varchar(20)} NOT NULL UNIQUE, name {varchar(100)} NOT NULL, region {varchar(20)} NOT NULL, email {varchar(160)}'),
        ('products', f'product_id INTEGER NOT NULL PRIMARY KEY, sku {varchar(20)} NOT NULL UNIQUE, name {varchar(100)} NOT NULL, unit_price {money} NOT NULL, active INTEGER NOT NULL'),
        ('orders', f'order_id INTEGER NOT NULL PRIMARY KEY, customer_id INTEGER NOT NULL, sales_rep_id INTEGER NOT NULL, ordered_on DATE NOT NULL, status {varchar(20)} NOT NULL, total_amount {money} NOT NULL, note {varchar(400)}, CONSTRAINT fk_order_customer FOREIGN KEY(customer_id) REFERENCES customers(customer_id), CONSTRAINT fk_order_employee FOREIGN KEY(sales_rep_id) REFERENCES employees(employee_id)'),
        ('order_items', f'order_id INTEGER NOT NULL, line_no INTEGER NOT NULL, product_id INTEGER NOT NULL, quantity INTEGER NOT NULL, unit_price {money} NOT NULL, CONSTRAINT pk_order_items PRIMARY KEY(order_id,line_no), CONSTRAINT fk_item_order FOREIGN KEY(order_id) REFERENCES orders(order_id), CONSTRAINT fk_item_product FOREIGN KEY(product_id) REFERENCES products(product_id)'),
        ('support_tickets', f'ticket_id INTEGER NOT NULL PRIMARY KEY, customer_code {varchar(20)} NOT NULL, subject {varchar(200)} NOT NULL, priority INTEGER NOT NULL'),
        ('exact_values', f'value_id INTEGER NOT NULL PRIMARY KEY, big_integer {"NUMBER(19)" if kind == "oracle" else "BIGINT"} NOT NULL, amount {exact} NOT NULL, optional_text {varchar(200)}'),
        ('demo_ready', f'id INTEGER NOT NULL PRIMARY KEY, dataset_version {varchar(40)} NOT NULL'),
    ]
    return tables


def dataset():
    rows = {'departments': [(1, '営業部'), (2, '開発部'), (3, '管理部')]}
    rows['employees'] = [(i, (i-1) % 3 + 1, None if i == 1 else 1,
                           f'検証担当 {i:02}', f'employee{i}@example.invalid', Decimal('250000.50')+i,
                           None if i % 2 else '引用符 " とカンマ, を含む備考') for i in range(1, 9)]
    rows['customers'] = [(i, f'C{i:04}', f'サンプル顧客 {i:02}', ['東京', '大阪', '名古屋'][(i-1) % 3],
                         None if i % 5 == 0 else f'customer{i}@example.invalid') for i in range(1, 25)]
    prices = [Decimal(x) for x in ['980.00','2500.50','12000.00','150.25','7000.00','390.00','45000.99','80.00']]
    rows['products'] = [(i, f'P{i:04}', f'検証商品 {i:02}', prices[i-1], 0 if i == 8 else 1) for i in range(1,9)]
    rows['orders'], rows['order_items'] = [], []
    for i in range(1,1201):
        items = [(i, n, (i+n) % 8 + 1, (i+n) % 4 + 1, prices[(i+n) % 8]) for n in range(1,4)]
        total = sum((r[3]*r[4] for r in items), Decimal(0))
        rows['orders'].append((i, 1 if i % 4 else i % 23 + 2, (i-1) % 8 + 1,
                               datetime.date(2026,10,1)+datetime.timedelta(days=(i-1)%21),
                               ['PAID','NEW','CANCELLED'][i%3], total,
                               '複数行の備考\n2行目' if i == 1 else None))
        rows['order_items'].extend(items)
    rows['support_tickets'] = [(i, f'C{i:04}', f'配送についての問い合わせ {i}', i % 3 + 1) for i in range(1,7)]
    rows['exact_values'] = [(1, 9223372036854775807, Decimal('12345678901234567890123.12345678'), None),
                            (2, -9223372036854775807, Decimal('-0.00000001'), ''),
                            (3, 9007199254740993, Decimal('99999999999999999999999.99999999'), '整数は2^53を超える')]
    rows['demo_ready'] = [(1, 'yydbmanager-demo-v1')]
    return rows


def literal(value, kind, exact=False):
    if value is None:
        return 'NULL'
    if isinstance(value, (int, Decimal)) and not exact:
        return str(value)
    if isinstance(value, datetime.date):
        return f"DATE '{value}'" if kind in ('oracle', 'postgres', 'duckdb') else f"'{value}'"
    s = str(value).replace("'", "''")
    if kind == 'mysql':
        s = s.replace('\\', '\\\\')
    return ('N' if kind == 'sqlserver' else '') + "'" + s + "'"


def generate_sql(kind, rows):
    out = ['-- Generated by tools/generate.py; deterministic demo data; UTF-8.']
    if kind == 'postgres':
        out += ['CREATE EXTENSION IF NOT EXISTS pg_stat_statements;', 'BEGIN;']
    elif kind == 'duckdb':
        out += ['BEGIN;']
    elif kind == 'mysql':
        out += ['SET NAMES utf8mb4;', 'USE yydemo;',
                "GRANT SELECT ON performance_schema.* TO 'yy_demo'@'%';",
                "GRANT SELECT ON sys.* TO 'yy_demo'@'%';",
                "GRANT EXECUTE ON FUNCTION sys.quote_identifier TO 'yy_demo'@'%';",
                "GRANT EXECUTE ON FUNCTION sys.format_statement TO 'yy_demo'@'%';",
                "GRANT EXECUTE ON FUNCTION sys.sys_get_config TO 'yy_demo'@'%';",
                "GRANT PROCESS ON *.* TO 'yy_demo'@'%';"]
    elif kind == 'sqlserver':
        out += ["IF DB_ID(N'yydemo') IS NULL CREATE DATABASE yydemo;", 'GO', 'USE yydemo;', 'GO',
                "IF NOT EXISTS(SELECT 1 FROM sys.server_principals WHERE name=N'yy_demo') CREATE LOGIN yy_demo WITH PASSWORD=N'YyDemo_2026!', CHECK_POLICY=OFF;",
                'GRANT VIEW SERVER PERFORMANCE STATE TO yy_demo;',
                'CREATE USER yy_demo FOR LOGIN yy_demo;', 'ALTER ROLE db_owner ADD MEMBER yy_demo;',
                'SET XACT_ABORT ON;', 'BEGIN TRANSACTION;']
    elif kind == 'oracle':
        out += ['WHENEVER SQLERROR EXIT SQL.SQLCODE ROLLBACK', 'ALTER SESSION SET CONTAINER=FREEPDB1;',
                "CREATE TABLESPACE yy_demo DATAFILE '/opt/oracle/oradata/yy_demo.dbf' SIZE 32M AUTOEXTEND ON NEXT 16M MAXSIZE 512M;",
                'CREATE USER yy_demo IDENTIFIED BY "YyDemo_2026!" DEFAULT TABLESPACE yy_demo QUOTA UNLIMITED ON yy_demo;',
                'GRANT CREATE SESSION, CREATE TABLE, CREATE VIEW, CREATE SEQUENCE, ALTER SESSION TO yy_demo;',
                'GRANT SELECT ON SYS.V_$PARAMETER TO yy_demo;', 'GRANT SELECT ON SYS.V_$SESSION TO yy_demo;',
                'GRANT SELECT ON SYS.V_$SQL TO yy_demo;',
                'GRANT SELECT ON SYS.V_$SQL_PLAN_STATISTICS_ALL TO yy_demo;',
                'ALTER SESSION SET CURRENT_SCHEMA=yy_demo;']
    elif kind == 'sqlite':
        out += ['PRAGMA foreign_keys=ON;', 'BEGIN;']
    for table, ddl in schema(kind):
        out += [f'CREATE TABLE {table} ({ddl});']
    out += ['CREATE INDEX ix_orders_customer_date ON orders(customer_id,ordered_on);',
            'CREATE INDEX ix_orders_status ON orders(status);',
            'CREATE INDEX ix_items_product ON order_items(product_id);',
            'CREATE INDEX ix_employee_department ON employees(department_id);',
            'CREATE INDEX ix_ticket_customer_code ON support_tickets(customer_code);']
    for table, records in rows.items():
        # Oracle Free supports INSERT ALL across the 23/26 images; SQL Server caps VALUES at 1000.
        for start in range(0,len(records),1 if kind == 'duckdb' and table == 'employees' else 100):
            chunk = records[start:start+(1 if kind == 'duckdb' and table == 'employees' else 100)]
            values = ['(' + ','.join(literal(v, kind, kind == 'sqlite' and table == 'exact_values' and c == 2)
                                     for c,v in enumerate(row)) + ')' for row in chunk]
            if kind == 'oracle':
                out += ['INSERT ALL\n' + '\n'.join(f'  INTO {table} VALUES {v}' for v in values) + '\nSELECT 1 FROM dual;']
            else:
                out += [f'INSERT INTO {table} VALUES\n' + ',\n'.join(values) + ';']
    if kind == 'sqlserver':
        out += ['GO']
    out += ['CREATE VIEW customer_sales AS SELECT c.customer_id,c.name,COUNT(o.order_id) AS order_count,COALESCE(SUM(o.total_amount),0) AS total_amount FROM customers c LEFT JOIN orders o ON o.customer_id=c.customer_id GROUP BY c.customer_id,c.name;']
    if kind == 'sqlserver':
        out += ['GO']
    if kind in ('postgres','sqlite','sqlserver','oracle','db2','duckdb'):
        out += ['COMMIT;']
    if kind == 'postgres':
        out += ['ANALYZE;']
    elif kind == 'mysql':
        out += ['ANALYZE TABLE customers,orders,order_items,products;']
    elif kind == 'sqlserver':
        out += ['EXEC sp_updatestats;', 'GO']
    elif kind == 'db2':
        out += [f"CALL SYSPROC.ADMIN_CMD('RUNSTATS ON TABLE DB2INST1.{t.upper()} WITH DISTRIBUTION AND DETAILED INDEXES ALL');" for t in ('CUSTOMERS','ORDERS','ORDER_ITEMS','PRODUCTS')]
        out += ["CALL SYSPROC.SYSINSTALLOBJECTS('EXPLAIN','C',CAST(NULL AS VARCHAR(128)),CURRENT SCHEMA);"]
    elif kind == 'oracle':
        out += ["BEGIN DBMS_STATS.GATHER_SCHEMA_STATS('YY_DEMO'); END;", '/', 'EXIT']
    return '\n'.join(out)+'\n'


def main():
    rows = dataset()
    for kind in KINDS:
        (ROOT/'init'/f'{kind}.sql').write_text(generate_sql(kind, rows), encoding='utf-8', newline='\n')
    (ROOT/'data'/'expected-counts.json').write_text(json.dumps(EXPECTED,indent=2)+'\n',encoding='utf-8')
    sample = [
        ['external_id','customer_name','amount','memo'],
        ['I001','株式会社サンプル','1234.50','カンマ, と "引用符"'],
        ['I002','大阪営業所','0.00','1行目\r\n2行目'],
        ['I003','空文字の確認','-12.34',''],
        ['I004','NULLの確認','999999.99','\\N'],
        ['I005','タブの確認','42.00','左\t右'],
    ]
    for ext, delim in [('csv',','),('tsv','\t'),('psv','|')]:
        with (ROOT/'data'/f'import_sample.{ext}').open('w',encoding='utf-8',newline='') as f:
            csv.writer(f,delimiter=delim,lineterminator='\r\n').writerows(sample)
    (ROOT/'data'/'fixed_sample.cbl').write_text('       01 DEMO-RECORD.\n          05 ITEM-ID PIC 9(5).\n          05 ITEM-NAME PIC X(20).\n          05 AMOUNT PIC 9(7)V99.\n',encoding='ascii')
    fixed = b''.join(f'{i:05}{name:<20}{amount:09}'.encode('ascii') for i,name,amount in [(1,'Notebook',123450),(2,'Mouse',250050),(3,'USB cable',98000)])
    (ROOT/'data'/'fixed_sample.dat').write_bytes(fixed)


if __name__ == '__main__':
    main()
