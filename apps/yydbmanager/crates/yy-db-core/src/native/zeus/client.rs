// Adapted from ykwyuta/Zeus, 99a30e6e0748357eb2aeecb7a584785d023e0935. GPL-3.0.
//! A DRDA connection to a Db2 database.

use std::io::Read;
use std::io::{BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::ebcdic;

use super::ddm::{self, CHAINED, Dss, OBJDSS, Object, RQSDSS, SAME_CORRELATOR, cp};

/// A DRDA protocol or server error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Error {
        Error(format!("I/O: {e}"))
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// DDM character parameters are EBCDIC.
pub(crate) fn ebc(s: &str) -> Vec<u8> {
    ebcdic::encode(s)
}

pub(crate) fn ebc_padded(s: &str, n: usize) -> Vec<u8> {
    let mut b = ebc(s);
    b.resize(n, ebcdic::SPACE);
    b
}

pub(crate) fn from_ebc(b: &[u8]) -> String {
    ebcdic::decode_latin1(b)
        .iter()
        .map(|&c| c as char)
        .collect::<String>()
        .trim_end()
        .to_string()
}

pub struct Connection {
    pub(crate) out: SharedStream,
    pub(crate) inp: BufReader<SharedStream>,
    socket: TcpStream,
    correlator: u16,
    pub rdbnam: String,
    /// Data representation of the server (TYPDEFNAM of ACCRDBRM).
    pub server_typdef: String,
    /// Server CCSIDs (single-byte, mixed).
    pub server_ccsid: (u16, u16),
    pub trace: bool,
    /// Package section used by the last query.
    pub(crate) section: u16,
}

/// Connection parameters.
#[derive(Clone)]
pub struct Params {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub user: String,
    pub password: String,
}

impl Params {
    /// `db2://user:password@host:port/DATABASE`
    pub fn from_url(url: &str) -> Result<Params> {
        let bad = || Error("bad Db2 URL (db2://user:password@host:port/DB)".into());
        let rest = url.strip_prefix("db2://").ok_or_else(bad)?;
        let (cred, rest) = rest.rsplit_once('@').ok_or_else(bad)?;
        let (user, password) = cred.split_once(':').ok_or_else(bad)?;
        let (hostport, db) = rest.split_once('/').ok_or_else(bad)?;
        let (host, port) = match hostport.rsplit_once(':') {
            Some((h, p)) => (h, p.parse().map_err(|_| bad())?),
            None => (hostport, 50000),
        };
        Ok(Params {
            host: host.to_string(),
            port,
            database: db.to_string(),
            user: user.to_string(),
            password: password.to_string(),
        })
    }
}

impl Connection {
    pub fn connect(p: &Params) -> Result<Connection> {
        Self::connect_tls(p, None)
    }

    pub fn connect_tls(p: &Params, tls: Option<native_tls::TlsConnector>) -> Result<Connection> {
        if !p.user.is_ascii() || !p.password.is_ascii() || !p.database.is_ascii() {
            return Err(Error(
                "Zeus DRDAの認証情報・DB名はASCIIで指定してください".into(),
            ));
        }
        if p.user.len() > 128 || p.password.len() > 128 || p.database.len() > 128 {
            return Err(Error("Db2接続情報が長すぎます".into()));
        }
        let mut socket = None;
        for address in (p.host.as_str(), p.port).to_socket_addrs()? {
            match TcpStream::connect_timeout(&address, Duration::from_secs(30)) {
                Ok(s) => {
                    socket = Some(s);
                    break;
                }
                Err(_) => continue,
            }
        }
        let s =
            socket.ok_or_else(|| Error("Db2への接続が30秒以内に確立できませんでした".into()))?;
        s.set_nodelay(true)?;
        s.set_read_timeout(Some(Duration::from_secs(60)))?;
        s.set_write_timeout(Some(Duration::from_secs(60)))?;
        let socket = s.try_clone()?;
        let io: Box<dyn ReadWrite> = if let Some(tls) = tls {
            Box::new(
                tls.connect(&p.host, s)
                    .map_err(|e| Error(format!("Db2 TLS: {e}")))?,
            )
        } else {
            Box::new(s)
        };
        let stream = SharedStream(Arc::new(Mutex::new(io)));
        let mut c = Connection {
            out: stream.clone(),
            inp: BufReader::new(stream),
            socket,
            correlator: 0,
            rdbnam: p.database.to_ascii_uppercase(),
            server_typdef: String::new(),
            server_ccsid: (1208, 1208),
            trace: false,
            section: 1,
        };
        c.handshake(p)?;
        Ok(c)
    }
    pub fn interrupt_socket(&self) -> Result<TcpStream> {
        Ok(self.socket.try_clone()?)
    }

    pub(crate) fn rdbnam_param(&self) -> Vec<u8> {
        ebc_padded(&self.rdbnam, 18.max(self.rdbnam.len()))
    }

    fn next_correlator(&mut self) -> u16 {
        self.correlator = self.correlator.wrapping_add(1).max(1);
        self.correlator
    }

    /// Sends a chain of requests: each a command with its command data
    /// objects. Returns the reply DSSs.
    pub(crate) fn exchange(&mut self, chain: &[(Object, Vec<Object>)]) -> Result<Vec<Dss>> {
        let mut buf = Vec::new();
        for (k, (cmd, objs)) in chain.iter().enumerate() {
            let corr = self.next_correlator();
            let last_cmd = k + 1 == chain.len();
            let mut fmt = RQSDSS;
            if !objs.is_empty() {
                fmt |= CHAINED | SAME_CORRELATOR;
            } else if !last_cmd {
                fmt |= CHAINED;
            }
            buf.extend(ddm::dss(fmt, corr, cmd));
            for (j, o) in objs.iter().enumerate() {
                let mut f = OBJDSS;
                if j + 1 < objs.len() {
                    f |= CHAINED | SAME_CORRELATOR;
                } else if !last_cmd {
                    f |= CHAINED;
                }
                buf.extend(ddm::dss(f, corr, o));
            }
        }
        // Never trace raw authentication packets (they include credentials).
        self.out.write_all(&buf)?;
        self.out.flush()?;
        let mut replies = Vec::new();
        loop {
            let d = ddm::read_dss(&mut self.inp)?;
            if self.trace {
                eprintln!(
                    "DRDA < fmt {:02X} corr {} cp {:04X} {}",
                    d.format,
                    d.correlator,
                    d.object.cp,
                    hex(&d.object.data)
                );
            }
            let more = d.chained();
            if replies.len() >= 4096
                || replies
                    .iter()
                    .map(|r: &Dss| r.object.data.len())
                    .sum::<usize>()
                    + d.object.data.len()
                    > 64 * 1024 * 1024
            {
                return Err(Error("Db2の応答が上限を超えています".into()));
            }
            replies.push(d);
            if !more {
                break;
            }
        }
        Ok(replies)
    }

    fn handshake(&mut self, p: &Params) -> Result<()> {
        let mut mgrs = Vec::new();
        for (m, l) in [
            (cp::AGENT, 7u16),
            (cp::SQLAM, 7),
            (cp::RDB, 7),
            (cp::SECMGR, 7),
            (cp::CMNTCPIP, 5),
        ] {
            mgrs.extend(m.to_be_bytes());
            mgrs.extend(l.to_be_bytes());
        }
        let excsat = Object::new(cp::EXCSAT)
            .param(cp::EXTNAM, &ebc("ZEUS"))
            .param(cp::SRVNAM, &ebc("ZEUS"))
            .param(cp::SRVRLSLV, &ebc("ZEU01000"))
            .param(cp::MGRLVLLS, &mgrs)
            .param(cp::SRVCLSNM, &ebc("ZEUS"));
        let accsec = Object::new(cp::ACCSEC)
            .param_u16(cp::SECMEC, 3)
            .param(cp::RDBNAM, &self.rdbnam_param());
        let r = self.exchange(&[(excsat, vec![]), (accsec, vec![])])?;
        check_errors(&r)?;
        let secchk = Object::new(cp::SECCHK)
            .param_u16(cp::SECMEC, 3)
            .param(cp::RDBNAM, &self.rdbnam_param())
            .param(cp::USRID, &ebc(&p.user))
            .param(cp::PASSWORD, &ebc(&p.password));
        let ovr = Object::new(cp::TYPDEFOVR)
            .param_u16(cp::CCSIDSBC, 1208)
            .param_u16(cp::CCSIDDBC, 1200)
            .param_u16(cp::CCSIDMBC, 1208);
        let accrdb = Object::new(cp::ACCRDB)
            .param(cp::RDBNAM, &self.rdbnam_param())
            .param_u16(cp::RDBACCCL, cp::SQLAM)
            .param(cp::PRDID, &ebc("ZEU01000"))
            .param(cp::TYPDEFNAM, &ebc("QTDSQLASC"))
            .param(cp::TYPDEFOVR, &ovr.data)
            .param(cp::CRRTKN, &ebc("ZEUS.CONN"));
        let r = self.exchange(&[(secchk, vec![]), (accrdb, vec![])])?;
        check_errors(&r)?;
        for d in &r {
            if d.object.cp == cp::SECCHKRM
                && let Some(code) = d.object.get(cp::SECCHKCD)
                && code.data.first().copied().unwrap_or(0) != 0
            {
                return Err(Error(format!(
                    "security check failed (SECCHKCD {:02X})",
                    code.data[0]
                )));
            }
            if d.object.cp == cp::ACCRDBRM {
                if let Some(t) = d.object.get(cp::TYPDEFNAM) {
                    self.server_typdef = from_ebc(&t.data);
                }
                if let Some(o) = d.object.get(cp::TYPDEFOVR) {
                    if let Some(s) = o.u16_of(cp::CCSIDSBC) {
                        self.server_ccsid.0 = s;
                    }
                    if let Some(m) = o.u16_of(cp::CCSIDMBC) {
                        self.server_ccsid.1 = m;
                    }
                }
            }
        }
        if !r.iter().any(|d| d.object.cp == cp::ACCRDBRM) {
            return Err(Error("no ACCRDBRM in the reply".into()));
        }
        Ok(())
    }
}

trait ReadWrite: Read + Write + Send {}
impl<T: Read + Write + Send> ReadWrite for T {}
#[derive(Clone)]
pub(crate) struct SharedStream(Arc<Mutex<Box<dyn ReadWrite>>>);
impl Read for SharedStream {
    fn read(&mut self, data: &mut [u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| std::io::Error::other("Db2 stream lock poisoned"))?
            .read(data)
    }
}
impl Write for SharedStream {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| std::io::Error::other("Db2 stream lock poisoned"))?
            .write(data)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0
            .lock()
            .map_err(|_| std::io::Error::other("Db2 stream lock poisoned"))?
            .flush()
    }
}

/// Fails on error reply messages (other than SQLERRRM, which the SQLCARD
/// describes).
pub(crate) fn check_errors(r: &[Dss]) -> Result<()> {
    for d in r {
        let name = match d.object.cp {
            cp::SYNTAXRM => "SYNTAXRM (syntax error)",
            cp::PRCCNVRM => "PRCCNVRM (conversation protocol error)",
            cp::CMDCHKRM => "CMDCHKRM (command check)",
            cp::OBJNSPRM => "OBJNSPRM (object not supported)",
            cp::PRMNSPRM => "PRMNSPRM (parameter not supported)",
            cp::VALNSPRM => "VALNSPRM (parameter value not supported)",
            cp::CMDNSPRM => "CMDNSPRM (command not supported)",
            cp::MGRLVLRM => "MGRLVLRM (manager level conflict)",
            cp::RDBNACRM => "RDBNACRM (database not accessed)",
            cp::RDBNFNRM => "RDBNFNRM (database not found)",
            _ => continue,
        };
        let detail = d
            .object
            .params()
            .iter()
            .map(|p| format!("{:04X}={}", p.cp, hex(&p.data)))
            .collect::<Vec<_>>()
            .join(" ");
        return Err(Error(format!("{name}: {detail}")));
    }
    Ok(())
}

pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_urls_do_not_disclose_credentials() {
        let error = Params::from_url("db2://user:private-password@bad:invalid/DB")
            .err()
            .unwrap();
        assert!(!error.0.contains("private-password"));
        assert_eq!(ebc("A1 "), vec![0xc1, 0xf1, 0x40]);
    }
}
