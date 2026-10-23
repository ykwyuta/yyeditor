// Adapted from ykwyuta/Zeus, 99a30e6e0748357eb2aeecb7a584785d023e0935. GPL-3.0.
//! SQL statements over a DRDA connection: dynamic SQL in the sections of
//! the NULLID.SYSSH200 package that Db2 provides for dynamic SQL.

use super::client::{Connection, Error, Result, ebc_padded, hex};
use super::ddm::{Dss, Object, cp};

/// The SQL communication area returned by the server.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SqlCa {
    pub sqlcode: i32,
    pub sqlstate: String,
    pub errproc: String,
    pub errd: [i32; 6],
    pub warn: String,
    /// Message tokens (SQLERRMC).
    pub tokens: Vec<String>,
}

impl SqlCa {
    pub fn ok() -> SqlCa {
        SqlCa {
            sqlstate: "00000".into(),
            ..SqlCa::default()
        }
    }
}

/// Reads an SQLCARD (SQLCAGRP); `le`: integers are little-endian.
pub fn parse_sqlca(d: &[u8], le: bool) -> SqlCa {
    parse_sqlca_len(d, le).0
}

/// An SQLCA and the bytes it takes (with its null indicator).
fn parse_sqlca_len(d: &[u8], le: bool) -> (SqlCa, usize) {
    if d.first() != Some(&0) {
        return (SqlCa::ok(), 1);
    }
    if d.len() < 19 {
        return (SqlCa::ok(), d.len());
    }
    let int = |b: &[u8]| -> i32 {
        let a = [b[0], b[1], b[2], b[3]];
        if le {
            i32::from_le_bytes(a)
        } else {
            i32::from_be_bytes(a)
        }
    };
    let s = |b: &[u8]| String::from_utf8_lossy(b).to_string();
    let mut ca = SqlCa {
        sqlcode: int(&d[1..5]),
        sqlstate: s(&d[5..10]),
        errproc: s(&d[10..18]),
        ..SqlCa::default()
    };
    // SQLCAXGRP
    let mut i = 18;
    if d.get(i) == Some(&0) {
        i += 1;
        for k in 0..6 {
            if i + 4 <= d.len() {
                ca.errd[k] = int(&d[i..i + 4]);
            }
            i += 4;
        }
        if i + 11 <= d.len() {
            ca.warn = s(&d[i..i + 11]);
        }
        i += 11;
        // SQLRDBNAME, SQLERRMSG_m, SQLERRMSG_s: 2-byte length + data
        let vc = |i: &mut usize| -> Vec<u8> {
            if *i + 2 > d.len() {
                return Vec::new();
            }
            let n = u16::from_be_bytes([d[*i], d[*i + 1]]) as usize;
            let v = d[*i + 2..(*i + 2 + n).min(d.len())].to_vec();
            *i += 2 + n;
            v
        };
        let _rdb = vc(&mut i);
        let m = vc(&mut i);
        let sm = vc(&mut i);
        let msg = if m.is_empty() { sm } else { m };
        ca.tokens = msg.split(|&b| b == 0xFF).map(s).collect();
    } else {
        i += 1;
    }
    (ca, i.min(d.len()))
}

/// A section of the dynamic SQL package.
pub(crate) fn pkgnamcsn(c: &Connection, section: u16) -> Vec<u8> {
    let mut v = c.rdbnam_param();
    v.extend(ebc_padded("NULLID", 18));
    v.extend(ebc_padded("SYSSH200", 18));
    // the consistency token is binary: these packages have "SYSLVL01"
    v.extend(b"SYSLVL01");
    v.extend(section.to_be_bytes());
    v
}

/// The SQLSTT object for a statement (UTF-8 text as the mixed string).
pub(crate) fn sqlstt(sql: &str) -> Object {
    let b = sql.as_bytes();
    let mut d = vec![0u8];
    d.extend((b.len() as u32).to_be_bytes());
    d.extend(b);
    d.push(0xFF);
    Object {
        cp: cp::SQLSTT,
        data: d,
    }
}

impl Connection {
    pub(crate) fn little_endian(&self) -> bool {
        self.server_typdef == "QTDSQLX86"
    }

    /// The SQLCA of a reply chain (SQLCARD or the SQLCA in an SQLDARD).
    pub(crate) fn reply_sqlca(&self, r: &[Dss]) -> SqlCa {
        for d in r {
            if d.object.cp == cp::SQLCARD || d.object.cp == cp::SQLDARD {
                return parse_sqlca(&d.object.data, self.little_endian());
            }
        }
        SqlCa::ok()
    }

    /// Executes a statement that returns no rows (EXCSQLIMM).
    pub fn execute(&mut self, sql: &str) -> Result<SqlCa> {
        if sql.len() > 30_000 {
            return Err(Error("Zeus DRDAのSQLは30,000バイト以内です".into()));
        }
        let cmd = Object::new(cp::EXCSQLIMM).param(cp::PKGNAMCSN, &pkgnamcsn(self, 1));
        let r = self.exchange(&[(cmd, vec![sqlstt(sql)])])?;
        super::client::check_errors(&r)?;
        Ok(self.reply_sqlca(&r))
    }

    pub fn commit(&mut self) -> Result<SqlCa> {
        let r = self.exchange(&[(Object::new(cp::RDBCMM), vec![])])?;
        super::client::check_errors(&r)?;
        Ok(self.reply_sqlca(&r))
    }

    pub fn rollback(&mut self) -> Result<SqlCa> {
        let r = self.exchange(&[(Object::new(cp::RDBRLLBCK), vec![])])?;
        super::client::check_errors(&r)?;
        Ok(self.reply_sqlca(&r))
    }

    /// Runs a query and returns all its rows (PRPSQLSTT, OPNQRY, then
    /// CNTQRY until the end of the data).
    pub fn query(&mut self, sql: &str) -> Result<Rows> {
        if sql.len() > 30_000 {
            return Err(Error("Zeus DRDAのSQLは30,000バイト以内です".into()));
        }
        self.section = if self.section >= 65 || self.section < 2 {
            2
        } else {
            self.section + 1
        };
        let pkg = pkgnamcsn(self, self.section);
        let prp = Object::new(cp::PRPSQLSTT)
            .param(cp::PKGNAMCSN, &pkg)
            .param(cp::RTNSQLDA, &[0xF1]);
        let r = self.exchange(&[(prp, vec![sqlstt(sql)])])?;
        super::client::check_errors(&r)?;
        let ca = self.reply_sqlca(&r);
        if ca.sqlcode < 0 {
            return Ok(Rows {
                sqlca: ca,
                ..Rows::default()
            });
        }
        let names = r
            .iter()
            .find(|d| d.object.cp == cp::SQLDARD)
            .map(|d| column_names(&d.object.data, self.little_endian()))
            .transpose()?
            .unwrap_or_default();
        let opn = Object::new(cp::OPNQRY)
            .param(cp::PKGNAMCSN, &pkg)
            .param(cp::QRYBLKSZ, &QRYBLKSZ.to_be_bytes());
        let mut r = self.exchange(&[(opn, vec![])])?;
        super::client::check_errors(&r)?;
        let mut rows = Rows::default();
        let Some(rm) = r.iter().find(|d| d.object.cp == cp::OPNQRYRM) else {
            rows.sqlca = self.reply_sqlca(&r);
            return Ok(rows);
        };
        let insid = rm.object.get(cp::QRYINSID).map(|o| o.data);
        let result: Result<Rows> = (|| {
            let Some(dsc) = r.iter().find(|d| d.object.cp == cp::QRYDSC) else {
                return Err(Error("OPNQRY reply without QRYDSC".into()));
            };
            rows.columns = parse_qrydsc(&dsc.object.data)?;
            if names.len() != rows.columns.len() {
                return Err(Error("Db2の列名と列数が一致しません".into()));
            }
            for (column, name) in rows.columns.iter_mut().zip(names) {
                column.name = name;
            }
            let mut data = Vec::new();
            loop {
                for d in &r {
                    if d.object.cp == cp::QRYDTA {
                        if data.len() + d.object.data.len() > 64 * 1024 * 1024 {
                            return Err(Error("Db2の問い合わせ結果が64MiBを超えました".into()));
                        }
                        data.extend(&d.object.data);
                    }
                }
                let warning = std::mem::take(&mut rows.sqlca);
                let (done, ca) = self.decode_rows(&data, &mut rows)?;
                if rows.sqlca.sqlcode == 0 && warning.sqlcode > 0 {
                    rows.sqlca = warning;
                }
                if done {
                    if ca.sqlcode != 100 || rows.sqlca.sqlcode <= 0 {
                        rows.sqlca = ca;
                    }
                    break;
                }
                if r.iter().any(|d| d.object.cp == cp::ENDQRYRM) {
                    rows.sqlca = self.reply_sqlca(&r);
                    break;
                }
                // more rows: continue the query
                let mut c = Object::new(cp::CNTQRY)
                    .param(cp::PKGNAMCSN, &pkg)
                    .param(cp::QRYBLKSZ, &QRYBLKSZ.to_be_bytes());
                if let Some(id) = &insid {
                    c = c.param(cp::QRYINSID, id);
                }
                r = self.exchange(&[(c, vec![])])?;
                super::client::check_errors(&r)?;
                if !r.iter().any(|d| d.object.cp == cp::QRYDTA) {
                    rows.sqlca = self.reply_sqlca(&r);
                    break;
                }
            }
            if rows.sqlca.sqlcode == 100 {
                rows.sqlca = SqlCa::ok();
            }
            Ok(rows)
        })();
        let cleanup: Result<()> = (|| {
            if !r.iter().any(|d| d.object.cp == cp::ENDQRYRM) {
                let mut close = Object::new(cp::CLSQRY).param(cp::PKGNAMCSN, &pkg);
                if let Some(id) = &insid {
                    close = close.param(cp::QRYINSID, id);
                }
                let closed = self.exchange(&[(close, vec![])])?;
                super::client::check_errors(&closed)?;
                let ca = self.reply_sqlca(&closed);
                if ca.sqlcode < 0 && ca.sqlcode != -501 {
                    return Err(Error(format!(
                        "cursor close SQLCODE={} SQLSTATE={}",
                        ca.sqlcode, ca.sqlstate
                    )));
                }
            }
            Ok(())
        })();
        match (result, cleanup) {
            (Err(e), Err(c)) => Err(Error(format!("{e}; cursor cleanup: {c}"))),
            (_, Err(c)) => Err(c),
            (result, Ok(())) => result,
        }
    }

    /// Decodes the complete rows in `data` (from the start, replacing what
    /// was decoded before); true when the end of the data was reached.
    fn decode_rows(&self, data: &[u8], rows: &mut Rows) -> Result<(bool, SqlCa)> {
        let le = self.little_endian();
        rows.rows.clear();
        let mut i = 0;
        while i < data.len() {
            match data[i] {
                // an SQLCA for the row: the end of the data, an error, or
                // a warning on the row that follows
                0x00 => {
                    let (ca, n) = parse_sqlca_len(&data[i..], le);
                    if ca.sqlcode == 100 || ca.sqlcode < 0 {
                        return Ok((true, ca));
                    }
                    i += n;
                    // a null SQLDIAGGRP
                    if data.get(i) == Some(&0xFF) && data.get(i + 1) == Some(&0) {
                        i += 1;
                    }
                    if rows.sqlca.sqlcode == 0 {
                        rows.sqlca = ca;
                    }
                }
                0xFF => i += 1,
                b => return Err(Error(format!("unexpected row indicator {b:02X}"))),
            }
            // the row (group) is present
            if data.get(i) != Some(&0) {
                i += 1;
                continue;
            }
            i += 1;
            let mut row = Vec::with_capacity(rows.columns.len());
            for c in &rows.columns {
                if c.nullable {
                    // a row continued in the next query block
                    let Some(&ind) = data.get(i) else {
                        return Ok((false, SqlCa::ok()));
                    };
                    i += 1;
                    if ind != 0 {
                        row.push(Value::Null);
                        continue;
                    }
                }
                match decode_value(c, data.get(i..).unwrap_or(&[]), le) {
                    Ok((v, n)) => {
                        row.push(v);
                        i += n;
                    }
                    Err(e) if e.0 == TRUNCATED => return Ok((false, SqlCa::ok())),
                    Err(e) => return Err(e),
                }
            }
            rows.rows.push(row);
        }
        Ok((false, SqlCa::ok()))
    }
}

/// SQLDARD descriptors supplement FD:OCA's wire types with column labels.
/// The alternate descriptor layout follows the MIT-licensed justybase-db2
/// 0.1.0-alpha.2 parser; see THIRD-PARTY-NOTICES.md.
fn column_names(data: &[u8], le: bool) -> Result<Vec<String>> {
    fn string(data: &[u8], at: &mut usize) -> Result<String> {
        let header = data
            .get(*at..*at + 2)
            .ok_or_else(|| Error("truncated SQLDARD string".into()))?;
        let n = u16::from_be_bytes([header[0], header[1]]) as usize;
        *at += 2;
        let bytes = data
            .get(*at..*at + n)
            .ok_or_else(|| Error("truncated SQLDARD string data".into()))?;
        *at += n;
        String::from_utf8(bytes.to_vec()).map_err(|e| Error(e.to_string()))
    }
    fn name(data: &[u8], at: &mut usize) -> Result<String> {
        let a = string(data, at)?;
        let b = string(data, at)?;
        Ok(if a.is_empty() { b } else { a })
    }
    let mut at = if data.first() == Some(&0xff) {
        1
    } else {
        parse_sqlca_len(data, le).1
    };
    if data.get(at) == Some(&0xff) {
        at += 1;
    }
    match data.get(at) {
        Some(0) => {
            at += 13;
            string(data, &mut at)?;
            name(data, &mut at)?;
        }
        Some(_) => at += 1,
        None => return Err(Error("SQLDARD header missing".into())),
    }
    let b = data
        .get(at..at + 2)
        .ok_or_else(|| Error("SQLDARD count missing".into()))?;
    at += 2;
    let count = if le {
        u16::from_le_bytes([b[0], b[1]])
    } else {
        u16::from_be_bytes([b[0], b[1]])
    };
    let mut names = vec![];
    for i in 0..count {
        if data.get(at..at + 16).is_none() {
            return Err(Error("SQLDARD fixed descriptor missing".into()));
        }
        at += 16;
        // SQLAM level 7 (the level negotiated by this client): three bytes
        // precede SQLNAME/SQLLABEL/SQLCOMMENTS, each a mixed/SBCS string pair.
        at += 3;
        let candidate = name(data, &mut at)?;
        let displayed = name(data, &mut at)?;
        name(data, &mut at)?;
        if data.get(at..at + 2) != Some(&[0xff, 0xff]) {
            return Err(Error("unsupported Db2 extended column descriptor".into()));
        }
        at += 2;
        let label = if displayed.is_empty() {
            candidate
        } else {
            displayed
        };
        names.push(if label.is_empty() {
            format!("COLUMN{}", i + 1)
        } else {
            label
        });
    }
    Ok(names)
}

/// The error of a row whose data continues in the next query block.
const TRUNCATED: &str = "row data truncated";

/// Query block size requested (bytes).
const QRYBLKSZ: u32 = 32767;

/// A column of a query result (FD:OCA description).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    /// DRDA data type (the nullable form is the odd code).
    pub drda_type: u8,
    pub nullable: bool,
    /// Length; for DECIMAL precision (high byte) and scale (low byte).
    pub len: u16,
    /// The column name, when the server describes it (else empty).
    pub name: String,
}

/// A column value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Int(i64),
    /// A decimal number as text: `-1234.50`.
    Dec(String),
    Float(f64),
    Str(String),
    Bytes(Vec<u8>),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Rows {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Value>>,
    pub sqlca: SqlCa,
}

/// The column descriptions of a QRYDSC: its X'76' triplets list the
/// columns as (type, length) entries.
fn parse_qrydsc(d: &[u8]) -> Result<Vec<Column>> {
    let mut cols = Vec::new();
    let mut i = 0;
    while i + 3 <= d.len() {
        let len = d[i] as usize;
        if len < 3 || i + len > d.len() {
            break;
        }
        if d[i + 1] == 0x76 {
            let mut k = i + 3;
            while k + 3 <= i + len {
                let t = d[k];
                cols.push(Column {
                    drda_type: t & 0xFE,
                    nullable: t & 1 == 1,
                    len: u16::from_be_bytes([d[k + 1], d[k + 2]]),
                    name: String::new(),
                });
                k += 3;
            }
        }
        i += len;
    }
    if cols.is_empty() {
        return Err(Error(format!("no columns in QRYDSC {}", hex(d))));
    }
    Ok(cols)
}

fn packed_text(b: &[u8], scale: usize) -> String {
    let mut digits = String::new();
    for (k, &x) in b.iter().enumerate() {
        digits.push((b'0' + (x >> 4)) as char);
        if k + 1 < b.len() {
            digits.push((b'0' + (x & 0x0F)) as char);
        }
    }
    let neg = matches!(b.last().map(|x| x & 0x0F), Some(0x0D | 0x0B));
    let digits = digits.trim_start_matches('0');
    let digits = format!("{digits:0>width$}", width = scale + 1);
    let (int, frac) = digits.split_at(digits.len() - scale);
    let mut s = String::new();
    if neg {
        s.push('-');
    }
    s.push_str(int);
    if scale > 0 {
        s.push('.');
        s.push_str(frac);
    }
    s
}

/// Decodes one value; returns it and the bytes used.
fn decode_value(c: &Column, d: &[u8], le: bool) -> Result<(Value, usize)> {
    let need = |n: usize| -> Result<()> {
        if d.len() < n {
            Err(Error(TRUNCATED.into()))
        } else {
            Ok(())
        }
    };
    let int = |n: usize| -> i64 {
        let mut b = [0u8; 8];
        if le {
            b[..n].copy_from_slice(&d[..n]);
            let v = i64::from_le_bytes(b);
            let shift = 64 - 8 * n as u32;
            (v << shift) >> shift
        } else {
            b[8 - n..].copy_from_slice(&d[..n]);
            let v = i64::from_be_bytes(b);
            let shift = 64 - 8 * n as u32;
            (v << shift) >> shift
        }
    };
    let text = |b: &[u8]| String::from_utf8_lossy(b).to_string();
    Ok(match c.drda_type {
        0x02 => {
            need(4)?;
            (Value::Int(int(4)), 4)
        }
        0x04 => {
            need(2)?;
            (Value::Int(int(2)), 2)
        }
        0x16 => {
            need(8)?;
            (Value::Int(int(8)), 8)
        }
        0x0E => {
            let p = (c.len >> 8) as usize;
            let s = (c.len & 0xFF) as usize;
            let n = p / 2 + 1;
            need(n)?;
            (Value::Dec(packed_text(&d[..n], s)), n)
        }
        0x0A => {
            need(8)?;
            let b = [d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]];
            let v = if le {
                f64::from_le_bytes(b)
            } else {
                f64::from_be_bytes(b)
            };
            (Value::Float(v), 8)
        }
        0x0C => {
            need(4)?;
            let b = [d[0], d[1], d[2], d[3]];
            let v = if le {
                f32::from_le_bytes(b)
            } else {
                f32::from_be_bytes(b)
            };
            (Value::Float(v as f64), 4)
        }
        // fixed-length character, dates and times
        0x30 | 0x3C | 0x20 | 0x22 | 0x24 => {
            let n = c.len as usize;
            need(n)?;
            (Value::Str(text(&d[..n])), n)
        }
        0x26 => {
            let n = c.len as usize;
            need(n)?;
            (Value::Bytes(d[..n].to_vec()), n)
        }
        // varying-length character / binary: 2-byte length
        0x32 | 0x3E | 0x28 | 0x34 | 0x40 => {
            need(2)?;
            let n = u16::from_be_bytes([d[0], d[1]]) as usize;
            need(2 + n)?;
            let v = if c.drda_type == 0x28 {
                Value::Bytes(d[2..2 + n].to_vec())
            } else {
                Value::Str(text(&d[2..2 + n]))
            };
            (v, 2 + n)
        }
        t => {
            return Err(Error(format!(
                "column type X'{t:02X}' (length {}) is not supported yet",
                c.len
            )));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sqlam7_column_names_from_real_db2_preserve_aliases() {
        let hex = "0000000000303030303053514C31323031350000000000000000000200000007000000000000000000000020202020202020202020200012595944454D4F20202020202020202020202000000000FFFF0300000000000800000000000000ED0100000000000002696400000000000000000000FFFF140004000000000000000000E50100000000000006616D6F756E7400000000000000000000FFFF000000000004000000000000C10104B800000000056C6162656C00000000000000000000FFFF";
        let bytes = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            column_names(&bytes, true).unwrap(),
            ["id", "amount", "label"]
        );
        for len in 0..bytes.len() {
            assert!(
                column_names(&bytes[..len], true).is_err(),
                "accepted truncated metadata at {len}"
            );
        }
    }

    #[test]
    fn packed_decimals() {
        assert_eq!(packed_text(&[0x00, 0x00, 0x01, 0x23, 0x4C], 2), "12.34");
        assert_eq!(packed_text(&[0x00, 0x00, 0x00, 0x50, 0x0D], 2), "-5.00");
        assert_eq!(packed_text(&[0x0C], 0), "0");
    }

    #[test]
    fn query_description() {
        let d = [
            0x18, 0x76, 0xD0, 0x02, 0x00, 0x04, 0x3F, 0x00, 0x14, 0x0F, 0x09, 0x02, 0x3D, 0x00,
            0x03, 0x21, 0x00, 0x0A, 0x05, 0x00, 0x02, 0x17, 0x00, 0x08, 0x09, 0x71, 0xE0, 0x54,
            0x00, 0x01, 0xD0, 0x00, 0x01,
        ];
        let c = parse_qrydsc(&d).unwrap();
        assert_eq!(c.len(), 7);
        assert_eq!(
            c[2],
            Column {
                drda_type: 0x0E,
                nullable: true,
                len: 0x0902,
                name: String::new(),
            }
        );
    }
}
