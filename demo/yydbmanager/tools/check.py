"""Validate checked-in demo files without Docker or third-party Python packages."""
from pathlib import Path
import csv
import json
import sqlite3

root=Path(__file__).resolve().parents[1]
expected=json.loads((root/'data/expected-counts.json').read_text(encoding='utf-8'))
with sqlite3.connect((root/'data/demo.sqlite').resolve().as_uri()+'?mode=ro', uri=True) as db:
    for table,count in expected.items():
        assert db.execute(f'SELECT COUNT(*) FROM {table}').fetchone()[0] == count,table
    assert db.execute('PRAGMA foreign_key_check').fetchall()==[]
    assert db.execute('SELECT COUNT(*) FROM orders WHERE customer_id=1').fetchone()[0]==900
    assert db.execute('SELECT amount FROM exact_values WHERE value_id=1').fetchone()[0]=='12345678901234567890123.12345678'
    assert db.execute('SELECT big_integer FROM exact_values WHERE value_id=1').fetchone()[0]==9223372036854775807
    assert db.execute("SELECT COUNT(*) FROM pragma_index_list('order_items') WHERE origin='pk'").fetchone()[0]==1
    for sql in (root/'sql').glob('0*.sql'):
        # Python's sqlite3 executes one statement at a time. These samples contain no semicolons in strings.
        source='\n'.join(line for line in sql.read_text(encoding='utf-8-sig').splitlines() if not line.lstrip().startswith('--'))
        for statement in source.split(';'):
            if statement.strip():
                db.execute(statement).fetchall()
    for statement in (root/'sql/performance/sqlite.sql').read_text(encoding='utf-8').split(';'):
        if statement.strip():
            db.execute(statement).fetchall()
help_text=(root.parents[1]/'apps/yydbmanager/help/help.md').read_text(encoding='utf-8')
for sql in (root/'sql/performance').glob('*.sql'):
    for statement in sql.read_text(encoding='utf-8').split(';'):
        assert statement.strip()+';' in help_text, f'Help/sample SQL mismatch: {sql.name}'
reference=None
for ext,delim in [('csv',','),('tsv','\t'),('psv','|')]:
    with (root/f'data/import_sample.{ext}').open(encoding='utf-8',newline='') as f:
        rows=list(csv.reader(f,delimiter=delim))
    assert len(rows)==6 and all(len(r)==4 for r in rows)
    if reference is None:reference=rows
    else:assert rows==reference
assert reference[2][3]=='1行目\r\n2行目'
assert reference[3][3]=='' and reference[4][3]=='\\N'
assert len((root/'data/fixed_sample.dat').read_bytes())==34*3
with (root/'data/column-definitions.tsv').open(encoding='utf-8',newline='') as f:
    assert all(len(r)==6 for r in csv.reader(f,delimiter='\t'))
for name in ['import_sample.parquet','exact_values.parquet']:
    data=(root/'data'/name).read_bytes()
    assert data[:4]==data[-4:]==b'PAR1'
print('Demo SQLite counts/foreign keys/precision/sample queries, CSV/TSV/PSV, grid TSV, fixed length and Parquet headers OK')
