#!/bin/bash
set -euo pipefail
export NLS_LANG=.AL32UTF8
# The official lite image runs startup hooks, not database-creation setup hooks.
if ! sqlplus -L -s 'yy_demo/"YyDemo_2026!"@//127.0.0.1:1521/FREEPDB1' @/demo/health.sql | grep -q yydbmanager-demo-v1; then
  sqlplus -L -s '/ as sysdba' @/demo/init.sql
fi
echo 'Demo: Oracle sample data ready'
