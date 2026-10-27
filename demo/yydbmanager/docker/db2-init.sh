#!/bin/bash
set -euo pipefail
if ! su - db2inst1 -c 'db2 connect to YYDEMO >/dev/null && db2 -x "SELECT dataset_version FROM demo_ready WHERE id=1"' 2>/dev/null | grep -q yydbmanager-demo-v1; then
  su - db2inst1 -c 'export DB2CODEPAGE=1208; db2 connect to YYDEMO && db2 -stvf /demo/init.sql'
fi
touch /database/yy-demo-ready
echo 'Demo: Db2 sample data ready'
