//! プリンターの LU: 端末の `PRINT` で届いたジョブを送り、PRINT-EOJ で終える。

use std::io;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

use yy_encoding::Ccsid;

use crate::codes::*;
use crate::ebcdic::{Screen, encode};
use crate::{Job, Link, Shared};

/// SCS の見本（日本語・改行・改ページ・SHF）。
pub(crate) fn scs_sample(ccsid: Ccsid) -> Vec<u8> {
    const NL: u8 = 0x15;
    const FF: u8 = 0x0C;
    // SHF: 1 行 80 桁
    let mut v = vec![0x2B, 0xC1, 0x02, 80];
    for line in [
        "請求書　No.0001",
        "品名        数量    金額",
        "ﾈｼﾞ M6       100   1,200",
    ] {
        v.extend(encode(ccsid, line));
        v.push(NL);
    }
    v.push(FF);
    v.extend(encode(ccsid, "２ページ目　END"));
    v.push(NL);
    v
}

/// LU3 の見本（80 桁の行で印刷する Erase/Write）。
pub(crate) fn lu3_sample(ccsid: Ccsid) -> Vec<u8> {
    let mut s = Screen::new(ccsid, 80);
    s.at(1, 1).text("LU3 の印刷 PAGE 1");
    s.at(2, 1).text("２行目 ABC");
    // WCC: 印刷開始・80 桁
    s.record(CMD_EW, WCC_RESET | WCC_START_PRINTER | 0x30)
}

pub(crate) fn run(link: &mut Link, shared: &Shared, rx: Receiver<Job>) -> io::Result<()> {
    let lu = link.lu.clone().unwrap_or_default();
    let mut jobs = 0;
    loop {
        // 応答・切断を見る
        link.recv(shared, Duration::from_millis(20))?;
        let job = match rx.recv_timeout(Duration::from_millis(80)) {
            Ok(j) => j,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        };
        jobs += 1;
        let kind = match &job {
            Job::Scs(d) => {
                if link.tn3270e {
                    link.send(shared, DT_SCS_DATA, d)?;
                } else {
                    shared.log(format!("scs needs TN3270E lu={lu}"));
                    continue;
                }
                "SCS"
            }
            Job::Lu3(d) => {
                link.send(shared, DT_3270_DATA, d)?;
                "LU3"
            }
        };
        if link.tn3270e {
            link.send(shared, DT_PRINT_EOJ, &[])?;
        }
        shared.log(format!("printed {kind} job={jobs} lu={lu}"));
    }
}
