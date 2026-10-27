//! Native wire-protocol connections. Rust database drivers.
mod query;
use crate::{DbKind, Result, ResultSet, Value};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tokio_util::compat::TokioAsyncWriteCompatExt;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
    pub password: String,
    #[serde(default = "default_tls")]
    pub tls: String,
    #[serde(default)]
    pub ca_file: Option<PathBuf>,
    #[serde(default)]
    pub trust_server_certificate: bool,
}
fn default_tls() -> String {
    "require".into()
}
impl Config {
    pub fn encode(&self) -> Result<String> {
        Ok(format!(
            "yynative:{}",
            serde_json::to_string(self).map_err(|e| e.to_string())?
        ))
    }
    pub fn decode(s: &str) -> Result<Option<Self>> {
        s.strip_prefix("yynative:")
            .map(|s| serde_json::from_str(s).map_err(|e| format!("直接接続設定が不正です: {e}")))
            .transpose()
    }
    pub fn validate(&self) -> Result<()> {
        if self.host.trim().is_empty()
            || self.database.is_empty()
            || self.port == 0
            || !["require", "disable", "prefer"].contains(&self.tls.as_str())
        {
            return Err("サーバー・DB名・ポート・TLS方式を確認してください".into());
        }
        Ok(())
    }
    fn tls_connector(&self) -> Result<native_tls::TlsConnector> {
        let mut builder = native_tls::TlsConnector::builder();
        builder.danger_accept_invalid_certs(self.trust_server_certificate);
        if let Some(path) = &self.ca_file {
            let data = std::fs::read(path).map_err(|e| e.to_string())?;
            builder.add_root_certificate(
                native_tls::Certificate::from_pem(&data).map_err(|e| e.to_string())?,
            );
        }
        builder.build().map_err(|e| e.to_string())
    }
}
type SqlServer = tiberius::Client<tokio_util::compat::Compat<tokio::net::TcpStream>>;
pub enum Driver {
    Postgres(postgres::Client),
    Mysql(mysql::Conn),
    SqlServer(Box<SqlServer>),
    Oracle(oracledb::Connection),
    Db2(rust_db2_driver::Connection),
}
pub struct Connection {
    pub(super) driver: Driver,
    runtime: tokio::runtime::Runtime,
    config: Config,
    kind: DbKind,
}
impl Connection {
    pub fn open(kind: DbKind, config: Config, read_only: bool) -> Result<Self> {
        config.validate()?;
        if read_only {
            return Err("直接接続ではDB側の読み取り専用権限を使用してください".into());
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let driver = match kind {
            DbKind::PostgreSql => {
                let mut c = postgres::Config::new();
                c.host(&config.host)
                    .port(config.port)
                    .dbname(&config.database)
                    .user(&config.username)
                    .password(&config.password)
                    .connect_timeout(Duration::from_secs(30));
                c.ssl_mode(match config.tls.as_str() {
                    "disable" => postgres::config::SslMode::Disable,
                    "prefer" => postgres::config::SslMode::Prefer,
                    _ => postgres::config::SslMode::Require,
                });
                Driver::Postgres(
                    c.connect(postgres_native_tls::MakeTlsConnector::new(
                        config.tls_connector()?,
                    ))
                    .map_err(|e| e.to_string())?,
                )
            }
            DbKind::MySql => {
                let ssl = if config.tls == "disable" {
                    None
                } else {
                    Some(
                        mysql::SslOpts::default()
                            .with_root_cert_path(config.ca_file.clone())
                            .with_danger_accept_invalid_certs(config.trust_server_certificate),
                    )
                };
                let options = mysql::OptsBuilder::default()
                    .ip_or_hostname(Some(&config.host))
                    .tcp_port(config.port)
                    .db_name(Some(&config.database))
                    .user(Some(&config.username))
                    .pass(Some(&config.password))
                    .ssl_opts(ssl)
                    .read_timeout(Some(Duration::from_secs(60)))
                    .write_timeout(Some(Duration::from_secs(60)))
                    .tcp_connect_timeout(Some(Duration::from_secs(30)));
                Driver::Mysql(mysql::Conn::new(options).map_err(|e| e.to_string())?)
            }
            DbKind::SqlServer => {
                if config.ca_file.is_some() {
                    return Err("SQL ServerのCA証明書はOSの信頼ストアに登録してください".into());
                }
                let mut c = tiberius::Config::new();
                c.host(&config.host);
                c.port(config.port);
                c.database(&config.database);
                if config.username.is_empty() {
                    #[cfg(windows)]
                    c.authentication(tiberius::AuthMethod::Integrated);
                    #[cfg(not(windows))]
                    return Err("SQL Serverの直接接続ではユーザー名を指定してください（Windows統合認証はWindows版で利用できます）".into());
                } else {
                    c.authentication(tiberius::AuthMethod::sql_server(
                        &config.username,
                        &config.password,
                    ));
                }
                if config.trust_server_certificate {
                    c.trust_cert();
                }
                c.encryption(if config.tls == "disable" {
                    tiberius::EncryptionLevel::NotSupported
                } else {
                    tiberius::EncryptionLevel::Required
                });
                let client = runtime.block_on(async {
                    let tcp = tokio::time::timeout(
                        Duration::from_secs(30),
                        tokio::net::TcpStream::connect(c.get_addr()),
                    )
                    .await
                    .map_err(|e| e.to_string())?
                    .map_err(|e| e.to_string())?;
                    tcp.set_nodelay(true).map_err(|e| e.to_string())?;
                    tiberius::Client::connect(c, tcp.compat_write())
                        .await
                        .map_err(|e| e.to_string())
                })?;
                Driver::SqlServer(Box::new(client))
            }
            DbKind::Oracle => {
                if config.ca_file.is_some() || config.trust_server_certificate {
                    return Err("OracleのCA証明書はOSの信頼ストアに登録してください（直接接続では証明書検証を無効化できません）".into());
                }
                let c = oracledb::Config::default()
                    .set_credentials(&config.username, &config.password)
                    .set_connect_string(&format!(
                        "{}{}:{}/{}",
                        if config.tls == "disable" {
                            ""
                        } else {
                            "tcps://"
                        },
                        config.host,
                        config.port,
                        config.database
                    ))
                    .map_err(|e| e.to_string())?;
                Driver::Oracle(oracledb::connect(c).map_err(|e| e.to_string())?)
            }
            DbKind::Db2 => {
                let p = rust_db2_driver::Params {
                    host: config.host.clone(),
                    port: config.port,
                    database: config.database.clone(),
                    user: config.username.clone(),
                    password: config.password.clone(),
                };
                Driver::Db2(db2_connect(&config, &p)?)
            }
            _ => return Err("SQLite/DuckDBは同梱エンジンを使用します".into()),
        };
        Ok(Self {
            driver,
            runtime,
            config,
            kind,
        })
    }
    pub fn interrupter(&self) -> Box<dyn Fn() + Send + Sync> {
        match &self.driver {
            Driver::Postgres(c) => {
                let token = c.cancel_token();
                let config = self.config.clone();
                Box::new(move || {
                    if let Ok(tls) = config.tls_connector() {
                        let _ = token.cancel_query(postgres_native_tls::MakeTlsConnector::new(tls));
                    }
                })
            }
            Driver::Db2(c) => {
                let handle = c.cancel_handle();
                Box::new(move || {
                    let _ = handle.cancel();
                })
            }
            _ => Box::new(|| {}),
        }
    }
    pub fn query(&mut self, sql: &str, cancel: &AtomicBool) -> Result<Vec<ResultSet>> {
        if cancel.load(Ordering::Relaxed) {
            return Err("取り消されました".into());
        }
        let result = query::run(&mut self.driver, &self.runtime, sql, cancel);
        result.map_err(|e| {
            if self.config.password.is_empty() {
                e
            } else {
                e.replace(&self.config.password, "***")
            }
        })
    }
    pub fn import(&self, set: &ResultSet, table: &str, create: bool) -> Result<u64> {
        let mut connection = Self::open(self.kind, self.config.clone(), false)?;
        let table = crate::sql::identifier_for(self.kind, table)?;
        let fields = set
            .columns
            .iter()
            .map(|c| crate::sql::identifier_for(self.kind, &c.name))
            .collect::<Result<Vec<_>>>()?;
        let cancel = AtomicBool::new(false);
        if self.kind == DbKind::PostgreSql {
            connection.query("SET standard_conforming_strings=on", &cancel)?;
        }
        if self.kind == DbKind::MySql {
            connection.query(
                "SET SESSION sql_mode=CONCAT(@@sql_mode,',NO_BACKSLASH_ESCAPES')",
                &cancel,
            )?;
        }
        if create {
            let defs = set
                .columns
                .iter()
                .zip(&fields)
                .map(|(c, f)| format!("{f} {}", column_type(self.kind, &c.type_name)))
                .collect::<Vec<_>>();
            connection.query(
                &format!("CREATE TABLE {table} ({})", defs.join(",")),
                &cancel,
            )?;
        }
        match self.kind {
            DbKind::Oracle | DbKind::Db2 => (),
            DbKind::SqlServer => {
                connection.query("BEGIN TRANSACTION", &cancel)?;
            }
            _ => {
                connection.query("BEGIN", &cancel)?;
            }
        }
        let result = (|| {
            let mut start = 0;
            while start < set.row_count {
                for (offset, row) in set.page(start, 256)?.into_iter().enumerate() {
                    if let Driver::Db2(c) = &mut connection.driver {
                        // The driver sends untyped NULL as VARCHAR, which Db2 cannot
                        // coerce to binary. SQL NULL lets the destination determine its type.
                        let markers = row
                            .iter()
                            .map(|v| if *v == Value::Null { "NULL" } else { "?" })
                            .collect::<Vec<_>>();
                        let params = row
                            .iter()
                            .filter(|v| **v != Value::Null)
                            .map(db2_parameter)
                            .collect::<Result<Vec<_>>>()?;
                        let sql = format!(
                            "INSERT INTO {table} ({}) VALUES ({})",
                            fields.join(","),
                            markers.join(",")
                        );
                        let ca = c.execute_params(&sql, &params).map_err(|e| e.to_string())?;
                        query::check_ca(&ca).map_err(|e| {
                            format!("インポートの{}行目: {e}", start + offset as u64 + 1)
                        })?;
                        continue;
                    }
                    let values = row
                        .iter()
                        .map(|v| literal(self.kind, v))
                        .collect::<Result<Vec<_>>>()?;
                    connection.query(
                        &format!(
                            "INSERT INTO {table} ({}) VALUES ({})",
                            fields.join(","),
                            values.join(",")
                        ),
                        &cancel,
                    )?;
                }
                start += 256;
            }
            connection.query("COMMIT", &cancel)?;
            Ok(set.row_count)
        })();
        if result.is_err() {
            let _ = connection.query("ROLLBACK", &cancel);
        }
        result
    }
}
fn db2_connect(
    config: &Config,
    params: &rust_db2_driver::Params,
) -> Result<rust_db2_driver::Connection> {
    validate_db2_tls(config)?;
    let result = if config.tls == "disable" {
        rust_db2_driver::Connection::connect(params)
    } else if let Some(path) = &config.ca_file {
        use rust_db2_driver::rustls::{
            self,
            pki_types::{CertificateDer, pem::PemObject},
        };
        let mut roots =
            rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let certs = CertificateDer::pem_file_iter(path).map_err(|e| e.to_string())?;
        let mut count = 0;
        for cert in certs {
            roots
                .add(cert.map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            count += 1;
        }
        if count == 0 {
            return Err("CAファイルに証明書がありません".into());
        }
        let tls = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_root_certificates(roots)
        .with_no_client_auth();
        rust_db2_driver::Connection::connect_with_tls(params, std::sync::Arc::new(tls))
    } else {
        rust_db2_driver::Connection::connect_tls(params)
    };
    result.map_err(|e| e.to_string())
}
pub fn validate_db2_tls(config: &Config) -> Result<()> {
    if config.trust_server_certificate {
        return Err(
            "Db2では証明書検証を省略できません。ca_fileでCA証明書を指定してください".into(),
        );
    }
    if config.tls == "prefer" {
        return Err("Db2のTLS方式はrequireまたはdisableを指定してください".into());
    }
    if config.tls == "disable" && config.ca_file.is_some() {
        return Err("Db2のCA証明書はTLS接続で指定してください".into());
    }
    Ok(())
}
fn db2_parameter(value: &Value) -> Result<rust_db2_driver::Value> {
    use rust_db2_driver::Value as V;
    Ok(match value {
        Value::Null => V::Null,
        Value::Integer(s) => V::Int(s.parse::<i64>().map_err(|e| e.to_string())?),
        Value::Decimal(s) => V::Dec(s.clone()),
        Value::Float(n) => V::Float(*n),
        Value::Bool(b) => V::Int(i64::from(*b)),
        Value::Text(s) | Value::Temporal(s) => V::Str(s.clone()),
        Value::Binary(b) => V::Bytes(b.clone()),
    })
}

fn column_type(kind: DbKind, t: &str) -> String {
    let t = t.to_ascii_lowercase();
    if t.contains("decimal") || t.contains("numeric") || t.contains("number") {
        let nums = t
            .split(|c: char| !c.is_ascii_digit())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        if nums.len() >= 2 {
            return format!("DECIMAL({},{})", nums[0], nums[1]);
        }
        return "DECIMAL(31,8)".into();
    }
    if t.contains("int") {
        return if kind == DbKind::Oracle {
            "NUMBER(19)"
        } else {
            "BIGINT"
        }
        .into();
    }
    if t.contains("binary") || t.contains("blob") || t.contains("bytea") || t.contains("raw") {
        return match kind {
            DbKind::PostgreSql => "BYTEA",
            DbKind::SqlServer => "VARBINARY(MAX)",
            DbKind::Db2 => "VARBINARY(1024)",
            _ => "BLOB",
        }
        .into();
    }
    match kind {
        DbKind::SqlServer => "NVARCHAR(1024)",
        DbKind::Oracle => "VARCHAR2(1024 CHAR)",
        _ => "VARCHAR(1024)",
    }
    .into()
}
fn literal(kind: DbKind, v: &Value) -> Result<String> {
    let quote = |s: &str| format!("'{}'", s.replace('\'', "''"));
    Ok(match v {
        Value::Null => "NULL".into(),
        Value::Integer(s) | Value::Decimal(s) => {
            if s.is_empty()
                || !s
                    .bytes()
                    .all(|b| b.is_ascii_digit() || b"+-.eE".contains(&b))
                || s.parse::<f64>().is_err()
            {
                return Err("数値の表現が不正です".into());
            }
            s.clone()
        }
        Value::Float(n) => {
            if !n.is_finite() {
                return Err("非有限の浮動小数点は直接インポートできません".into());
            }
            n.to_string()
        }
        Value::Bool(b) => if kind == DbKind::PostgreSql {
            if *b { "TRUE" } else { "FALSE" }
        } else if *b {
            "1"
        } else {
            "0"
        }
        .into(),
        Value::Text(s) | Value::Temporal(s) => {
            if kind == DbKind::SqlServer {
                format!("N{}", quote(s))
            } else {
                quote(s)
            }
        }
        Value::Binary(b) => {
            let h = b.iter().map(|b| format!("{b:02X}")).collect::<String>();
            match kind {
                DbKind::PostgreSql => format!("decode('{h}','hex')"),
                DbKind::SqlServer => format!("0x{h}"),
                DbKind::Oracle => format!("HEXTORAW('{h}')"),
                DbKind::Db2 => format!("BX'{h}'"),
                _ => format!("X'{h}'"),
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn db2_bound_values_preserve_precision_unicode_and_binary() {
        use rust_db2_driver::Value as V;
        for (value, expected) in [
            (Value::Null, V::Null),
            (Value::Integer(i64::MAX.to_string()), V::Int(i64::MAX)),
            (
                Value::Decimal("12345678901234567890123.12345678".into()),
                V::Dec("12345678901234567890123.12345678".into()),
            ),
            (
                Value::Text("日本語'); DELETE FROM t; -- ?".into()),
                V::Str("日本語'); DELETE FROM t; -- ?".into()),
            ),
            (Value::Binary(vec![0, 255]), V::Bytes(vec![0, 255])),
        ] {
            assert_eq!(db2_parameter(&value).unwrap(), expected);
        }
        assert!(db2_parameter(&Value::Integer("1;DELETE FROM t".into())).is_err());
    }
    #[test]
    fn db2_tls_rejects_unsupported_settings_before_connecting() {
        let mut config = Config {
            host: "localhost".into(),
            port: 50001,
            database: "YYTEST".into(),
            username: "db2inst1".into(),
            password: "secret".into(),
            tls: "require".into(),
            ca_file: None,
            trust_server_certificate: false,
        };
        validate_db2_tls(&config).unwrap();
        config.tls = "prefer".into();
        assert!(validate_db2_tls(&config).is_err());
        config.tls = "require".into();
        config.trust_server_certificate = true;
        assert!(validate_db2_tls(&config).is_err());
        config.trust_server_certificate = false;
        config.ca_file = Some("test-ca.pem".into());
        validate_db2_tls(&config).unwrap();
        config.tls = "disable".into();
        assert!(validate_db2_tls(&config).is_err());
    }
    #[test]
    fn profiles_roundtrip_credentials_and_explicit_transport_settings() {
        let original = Config {
            host: "localhost".into(),
            port: 5432,
            database: "日本語 DB".into(),
            username: "user".into(),
            password: "pass;'}\\word".into(),
            tls: "require".into(),
            ca_file: Some("C:/certs/ca.pem".into()),
            trust_server_certificate: false,
        };
        let encoded = original.encode().unwrap();
        let loaded = Config::decode(&encoded).unwrap().unwrap();
        assert_eq!(loaded.password, original.password);
        assert_eq!(loaded.database, original.database);
        loaded.validate().unwrap();
        assert!(Config::decode("Driver={old};").unwrap().is_none());
        assert!(Config::decode("yynative:bad").is_err());
    }
    #[test]
    fn import_literals_never_round_exact_numeric_values_or_interpret_text_as_sql() {
        assert_eq!(
            literal(
                DbKind::PostgreSql,
                &Value::Decimal("1234567890123456.1234".into())
            )
            .unwrap(),
            "1234567890123456.1234"
        );
        assert_eq!(
            literal(
                DbKind::SqlServer,
                &Value::Text("x'); DELETE FROM t;--".into())
            )
            .unwrap(),
            "N'x''); DELETE FROM t;--'"
        );
        assert!(
            literal(
                DbKind::PostgreSql,
                &Value::Integer("1;DELETE FROM t".into())
            )
            .is_err()
        );
        assert_eq!(
            literal(DbKind::PostgreSql, &Value::Binary(vec![0, 255])).unwrap(),
            "decode('00FF','hex')"
        );
    }
}
