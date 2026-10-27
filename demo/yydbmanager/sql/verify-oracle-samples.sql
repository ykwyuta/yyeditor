WHENEVER SQLERROR EXIT SQL.SQLCODE ROLLBACK
WHENEVER OSERROR EXIT FAILURE
SET LINESIZE 240
SET PAGESIZE 100
@/demo/sql/01-overview.sql
@/demo/sql/02-join-and-plan.sql
@/demo/sql/03-index-selectivity.sql
@/demo/sql/04-exact-values-and-logical-relation.sql
@/demo/sql/performance/oracle.sql
BEGIN
  DECLARE n NUMBER;
  BEGIN
    SELECT COUNT(*) INTO n FROM departments WHERE name=UNISTR('\55B6\696D\90E8');
    IF n <> 1 THEN RAISE_APPLICATION_ERROR(-20001,'UTF-8 seed data mismatch'); END IF;
  END;
END;
/
EXIT SUCCESS
