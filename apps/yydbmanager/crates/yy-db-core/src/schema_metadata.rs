use crate::DbKind;
// Normalized rows: schema, table, constraint, P/U/F, ordinal, column,
// referenced schema, table, column. Catalog ordinals pair composite FK columns.
pub(super) fn key_sql(kind: DbKind) -> &'static str {
    match kind {
        DbKind::Sqlite => unreachable!("SQLite uses table-valued pragmas"),
        DbKind::DuckDb => {
            r#"SELECT schema_name,table_name,CAST(constraint_index AS VARCHAR),CASE constraint_type WHEN 'PRIMARY KEY' THEN 'P' WHEN 'UNIQUE' THEN 'U' ELSE 'F' END,generate_subscripts(constraint_column_names,1) AS ordinal,unnest(constraint_column_names),CASE WHEN constraint_type='FOREIGN KEY' THEN schema_name ELSE '' END,referenced_table,referenced_column_names[ordinal] FROM duckdb_constraints() WHERE database_name=current_database() AND constraint_type IN ('PRIMARY KEY','UNIQUE','FOREIGN KEY')"#
        }
        DbKind::PostgreSql => {
            r#"SELECT n.nspname,t.relname,c.conname,CASE c.contype WHEN 'p' THEN 'P' WHEN 'u' THEN 'U' ELSE 'F' END,k.ordinality,a.attname,pn.nspname,pt.relname,pa.attname FROM pg_catalog.pg_constraint c JOIN pg_catalog.pg_class t ON t.oid=c.conrelid JOIN pg_catalog.pg_namespace n ON n.oid=t.relnamespace CROSS JOIN LATERAL unnest(c.conkey) WITH ORDINALITY k(attnum,ordinality) JOIN pg_catalog.pg_attribute a ON a.attrelid=t.oid AND a.attnum=k.attnum LEFT JOIN pg_catalog.pg_class pt ON pt.oid=c.confrelid LEFT JOIN pg_catalog.pg_namespace pn ON pn.oid=pt.relnamespace LEFT JOIN pg_catalog.pg_attribute pa ON pa.attrelid=pt.oid AND pa.attnum=c.confkey[k.ordinality] WHERE c.contype IN ('p','u','f') AND n.nspname NOT LIKE 'pg_%' AND n.nspname<>'information_schema' ORDER BY 1,2,3,5"#
        }
        DbKind::MySql => {
            r#"SELECT k.table_schema,k.table_name,k.constraint_name,CASE c.constraint_type WHEN 'PRIMARY KEY' THEN 'P' WHEN 'UNIQUE' THEN 'U' ELSE 'F' END,k.ordinal_position,k.column_name,k.referenced_table_schema,k.referenced_table_name,k.referenced_column_name FROM information_schema.key_column_usage k JOIN information_schema.table_constraints c ON c.constraint_schema=k.constraint_schema AND c.table_name=k.table_name AND c.constraint_name=k.constraint_name WHERE k.table_schema=DATABASE() AND c.constraint_type IN ('PRIMARY KEY','UNIQUE','FOREIGN KEY') ORDER BY 1,2,3,5"#
        }
        DbKind::SqlServer => {
            r#"SELECT SCHEMA_NAME(t.schema_id),t.name,i.name,CASE WHEN i.is_primary_key=1 THEN 'P' ELSE 'U' END,ic.key_ordinal,c.name,CAST(NULL AS nvarchar(128)),CAST(NULL AS nvarchar(128)),CAST(NULL AS nvarchar(128)) FROM sys.indexes i JOIN sys.tables t ON t.object_id=i.object_id JOIN sys.index_columns ic ON ic.object_id=i.object_id AND ic.index_id=i.index_id JOIN sys.columns c ON c.object_id=t.object_id AND c.column_id=ic.column_id WHERE i.is_unique=1 AND i.has_filter=0 AND ic.key_ordinal>0 AND t.is_ms_shipped=0 UNION ALL SELECT SCHEMA_NAME(t.schema_id),t.name,f.name,'F',fc.constraint_column_id,c.name,SCHEMA_NAME(pt.schema_id),pt.name,pc.name FROM sys.foreign_keys f JOIN sys.foreign_key_columns fc ON fc.constraint_object_id=f.object_id JOIN sys.tables t ON t.object_id=f.parent_object_id JOIN sys.columns c ON c.object_id=t.object_id AND c.column_id=fc.parent_column_id JOIN sys.tables pt ON pt.object_id=f.referenced_object_id JOIN sys.columns pc ON pc.object_id=pt.object_id AND pc.column_id=fc.referenced_column_id WHERE f.is_disabled=0 AND f.is_not_trusted=0 ORDER BY 1,2,3,5"#
        }
        DbKind::Oracle => {
            r#"SELECT c.owner,c.table_name,c.constraint_name,CASE c.constraint_type WHEN 'R' THEN 'F' ELSE c.constraint_type END,k.position,k.column_name,p.owner,p.table_name,pk.column_name FROM all_constraints c JOIN all_cons_columns k ON k.owner=c.owner AND k.constraint_name=c.constraint_name LEFT JOIN all_constraints p ON p.owner=c.r_owner AND p.constraint_name=c.r_constraint_name LEFT JOIN all_cons_columns pk ON pk.owner=p.owner AND pk.constraint_name=p.constraint_name AND pk.position=k.position WHERE c.owner=SYS_CONTEXT('USERENV','CURRENT_SCHEMA') AND c.constraint_type IN ('P','U','R') AND c.status='ENABLED' AND c.validated='VALIDATED' ORDER BY 1,2,3,5"#
        }
        DbKind::Db2 => {
            r#"SELECT c.tabschema,c.tabname,c.constname,CASE c.type WHEN 'P' THEN 'P' WHEN 'U' THEN 'U' ELSE 'F' END,k.colseq,k.colname,r.reftabschema,r.reftabname,pk.colname FROM syscat.tabconst c JOIN syscat.keycoluse k ON k.tabschema=c.tabschema AND k.tabname=c.tabname AND k.constname=c.constname LEFT JOIN syscat.references r ON r.tabschema=c.tabschema AND r.tabname=c.tabname AND r.constname=c.constname LEFT JOIN syscat.keycoluse pk ON pk.tabschema=r.reftabschema AND pk.tabname=r.reftabname AND pk.constname=r.refkeyname AND pk.colseq=k.colseq WHERE c.type IN ('P','U','F') AND c.enforced='Y' AND c.tabschema NOT LIKE 'SYS%' ORDER BY 1,2,3,5"#
        }
    }
}
