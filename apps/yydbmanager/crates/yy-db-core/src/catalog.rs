use crate::{Database, DbKind, Result};
use std::sync::atomic::AtomicBool;

#[derive(Clone, Debug)]
pub struct Object {
    pub schema: String,
    pub kind: String,
    pub name: String,
}
#[derive(Clone, Debug)]
pub struct Column {
    pub name: String,
    pub data_type: String,
    pub nullable: String,
}
fn literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}
impl Database {
    pub fn catalog(&mut self) -> Result<Vec<Object>> {
        let sql = match self.kind() {
            DbKind::Sqlite => {
                "SELECT 'main',upper(type),name FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY 1,2,3"
            }
            DbKind::DuckDb => {
                "SELECT table_schema, CASE WHEN table_type='VIEW' THEN 'VIEW' ELSE 'TABLE' END,table_name FROM information_schema.tables UNION ALL SELECT schema_name,'INDEX',index_name FROM duckdb_indexes() UNION ALL SELECT schema_name,'SEQUENCE',sequence_name FROM duckdb_sequences() ORDER BY 1,2,3"
            }
            DbKind::PostgreSql => {
                "SELECT n.nspname,CASE c.relkind WHEN 'r' THEN 'TABLE' WHEN 'p' THEN 'TABLE' WHEN 'v' THEN 'VIEW' WHEN 'm' THEN 'VIEW' WHEN 'i' THEN 'INDEX' WHEN 'I' THEN 'INDEX' WHEN 'S' THEN 'SEQUENCE' END,c.relname FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE c.relkind IN ('r','p','v','m','i','I','S') AND n.nspname NOT LIKE 'pg_%' AND n.nspname<>'information_schema' UNION SELECT n.nspname,CASE p.prokind WHEN 'p' THEN 'PROCEDURE' ELSE 'FUNCTION' END,p.proname FROM pg_catalog.pg_proc p JOIN pg_catalog.pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname NOT LIKE 'pg_%' AND n.nspname<>'information_schema' ORDER BY 1,2,3"
            }
            DbKind::MySql => {
                "SELECT table_schema,CASE WHEN table_type='VIEW' THEN 'VIEW' ELSE 'TABLE' END,table_name FROM information_schema.tables WHERE table_schema=DATABASE() UNION ALL SELECT routine_schema,routine_type,routine_name FROM information_schema.routines WHERE routine_schema=DATABASE() UNION SELECT table_schema,'INDEX',CONCAT(table_name,'.',index_name) FROM information_schema.statistics WHERE table_schema=DATABASE() UNION ALL SELECT trigger_schema,'TRIGGER',trigger_name FROM information_schema.triggers WHERE trigger_schema=DATABASE() ORDER BY 1,2,3"
            }
            DbKind::SqlServer => {
                "SELECT SCHEMA_NAME(schema_id),CASE type WHEN 'U' THEN 'TABLE' WHEN 'V' THEN 'VIEW' WHEN 'P' THEN 'PROCEDURE' WHEN 'SO' THEN 'SEQUENCE' WHEN 'TR' THEN 'TRIGGER' ELSE 'FUNCTION' END,name FROM sys.objects WHERE is_ms_shipped=0 AND type IN ('U','V','P','SO','TR','FN','IF','TF') UNION ALL SELECT SCHEMA_NAME(o.schema_id),'INDEX',o.name+'.'+i.name FROM sys.indexes i JOIN sys.objects o ON o.object_id=i.object_id WHERE i.name IS NOT NULL AND o.is_ms_shipped=0 ORDER BY 1,2,3"
            }
            DbKind::Oracle => {
                "SELECT owner,object_type,object_name FROM all_objects WHERE owner=SYS_CONTEXT('USERENV','CURRENT_SCHEMA') AND object_type IN ('TABLE','VIEW','INDEX','SEQUENCE','PROCEDURE','FUNCTION','TRIGGER','PACKAGE') ORDER BY 1,2,3"
            }
            DbKind::Db2 => {
                "SELECT tabschema,CASE type WHEN 'V' THEN 'VIEW' ELSE 'TABLE' END,tabname FROM syscat.tables WHERE tabschema NOT LIKE 'SYS%' UNION ALL SELECT indschema,'INDEX',indname FROM syscat.indexes WHERE indschema NOT LIKE 'SYS%' UNION SELECT routineschema,CASE routinetype WHEN 'P' THEN 'PROCEDURE' ELSE 'FUNCTION' END,routinename FROM syscat.routines WHERE routineschema NOT LIKE 'SYS%' UNION ALL SELECT seqschema,'SEQUENCE',seqname FROM syscat.sequences WHERE seqschema NOT LIKE 'SYS%' ORDER BY 1,2,3"
            }
        };
        let sets = self.execute(sql, &AtomicBool::new(false))?;
        let mut objects = vec![];
        if let Some(set) = sets.first() {
            let mut start = 0;
            while start < set.row_count {
                for row in set.page(start, 512)? {
                    objects.push(Object {
                        schema: row[0].display().trim_end().into(),
                        kind: row[1].display().trim_end().into(),
                        name: row[2].display().trim_end().into(),
                    });
                }
                start += 512;
            }
        }
        Ok(objects)
    }
    pub fn object_columns(&mut self, schema: &str, table: &str) -> Result<Vec<Column>> {
        let schema_literal = literal(schema);
        let table_literal = literal(table);
        let sql = match self.kind() {
            DbKind::Sqlite => format!(
                "SELECT name,type,CASE WHEN \"notnull\"=1 THEN 'NO' ELSE 'YES' END FROM pragma_table_info({table_literal},{schema_literal}) ORDER BY cid"
            ),
            DbKind::Oracle => format!(
                "SELECT column_name,data_type,nullable FROM all_tab_columns WHERE owner={schema_literal} AND table_name={table_literal} ORDER BY column_id"
            ),
            DbKind::Db2 => format!(
                "SELECT colname,typename,nulls FROM syscat.columns WHERE tabschema={schema_literal} AND tabname={table_literal} ORDER BY colno"
            ),
            _ => format!(
                "SELECT column_name,data_type,is_nullable FROM information_schema.columns WHERE table_schema={schema_literal} AND table_name={table_literal} ORDER BY ordinal_position"
            ),
        };
        let sets = self.execute(&sql, &AtomicBool::new(false))?;
        let mut columns = vec![];
        if let Some(set) = sets.first() {
            let mut start = 0;
            while start < set.row_count {
                for row in set.page(start, 512)? {
                    columns.push(Column {
                        name: row[0].display().trim_end().into(),
                        data_type: row[1].display().trim_end().into(),
                        nullable: row[2].display().trim_end().into(),
                    });
                }
                start += 512;
            }
        }
        Ok(columns)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn embedded_catalog_and_quoted_columns() {
        for kind in [DbKind::Sqlite, DbKind::DuckDb] {
            let mut db = Database::open(&crate::Options::memory(kind)).unwrap();
            db.execute("CREATE TABLE \"odd'table\" (id INTEGER NOT NULL, name VARCHAR); CREATE VIEW people AS SELECT * FROM \"odd'table\"; CREATE INDEX by_id ON \"odd'table\"(id)",&AtomicBool::new(false)).unwrap();
            let objects = db.catalog().unwrap();
            for category in ["TABLE", "VIEW", "INDEX"] {
                assert!(objects.iter().any(|o| o.kind == category));
            }
            let columns = db.object_columns("main", "odd'table").unwrap();
            assert_eq!(columns.len(), 2);
            assert_eq!(columns[0].name, "id");
            assert_eq!(columns[0].nullable, "NO");
        }
    }
}
