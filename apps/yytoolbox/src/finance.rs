use crate::{Output, Params, Result, checkpoint, integer, number, value};

fn decimals(params: &Params) -> Result<u32> {
    let n = integer(params, "decimals", 0, 2)?;
    if n == 1 {
        Err("金額の小数桁は0または2です".into())
    } else {
        Ok(n)
    }
}
fn amount(n: f64, decimals: u32) -> String {
    let scale = 10f64.powi(decimals as i32);
    let n = (n * scale).round() / scale;
    format!("{:.*}", decimals as usize, if n == 0.0 { 0.0 } else { n })
}
fn units(n: i64, decimals: u32) -> String {
    if decimals == 0 {
        n.to_string()
    } else {
        format!("{}.{:02}", n / 100, n % 100)
    }
}

pub(crate) fn compound(params: &Params) -> Result<Output> {
    let principal = number(params, "principal", 0.0, 1e12)?;
    let annual = number(params, "rate", 0.0, 100.0)? / 100.0;
    let years = integer(params, "years", 1, 100)?;
    let periods = integer(params, "periods", 1, 365)?;
    if ![1, 2, 4, 12, 52, 365].contains(&periods) {
        return Err("複利回数は1・2・4・12・52・365を指定してください".into());
    }
    let deposit = number(params, "contribution", 0.0, 1e12)?;
    let decimals = decimals(params)?;
    let rate = annual / f64::from(periods);
    let beginning = match value(params, "timing", "期末") {
        "期首" => true,
        "期末" => false,
        _ => return Err("積立時点が不正です".into()),
    };
    let mut rows = vec![vec![
        "経過年".into(),
        "元金と積立の合計".into(),
        "運用益（税引前）".into(),
        "残高".into(),
    ]];
    let mut final_value = principal;
    for year in 0..=years {
        let n = f64::from(year) * f64::from(periods);
        let total = principal + deposit * n;
        let growth = (n * rate.ln_1p()).exp();
        let annuity = if rate == 0.0 {
            n
        } else {
            (n * rate.ln_1p()).exp_m1() / rate
        };
        let future =
            principal * growth + deposit * annuity * if beginning { 1.0 + rate } else { 1.0 };
        if !future.is_finite() {
            return Err("計算結果が数値の範囲を超えました".into());
        }
        final_value = future;
        rows.push(vec![
            year.to_string(),
            amount(total, decimals),
            amount(future - total, decimals),
            amount(future, decimals),
        ]);
    }
    Output::csv(
        rows,
        format!(
            "{years}年後の残高 {}。一定名目年率・年{periods}回複利。税金・手数料なし",
            amount(final_value, decimals)
        ),
    )
}

fn principal_units(text: &str, decimals: u32) -> Result<i64> {
    let parts: Vec<_> = text.trim().split('.').collect();
    if parts.len() > 2
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err("元金は桁区切りなしの正の金額で指定してください".into());
    }
    let whole = parts[0].parse::<i64>().map_err(|_| "元金が大きすぎます")?;
    if whole > 1_000_000_000_000 {
        return Err("元金は1兆以下です".into());
    }
    let fraction = parts.get(1).copied().unwrap_or("");
    if fraction.len() > decimals as usize {
        return Err("元金の小数桁が指定を超えています".into());
    }
    let fraction = if fraction.is_empty() {
        0
    } else {
        fraction.parse::<i64>().map_err(|_| "金額が不正です")?
            * 10i64.pow(decimals - fraction.len() as u32)
    };
    let total = whole * 10i64.pow(decimals) + fraction;
    if total > 1_000_000_000_000 * 10i64.pow(decimals) {
        return Err("元金は1兆以下です".into());
    }
    if total <= 0 {
        return Err("元金は0より大きい金額を指定してください".into());
    }
    Ok(total)
}

pub(crate) fn loan(params: &Params, cancelled: &impl Fn() -> bool) -> Result<Output> {
    let decimals = decimals(params)?;
    let principal = principal_units(value(params, "principal", ""), decimals)?;
    let rate = number(params, "rate", 0.0, 100.0)? / 100.0 / 12.0;
    let months = integer(params, "months", 1, 1200)?;
    let equal_principal = match value(params, "method", "元利均等") {
        "元利均等" => false,
        "元金均等" => true,
        _ => return Err("返済方式が不正です".into()),
    };
    let payment = if rate == 0.0 {
        principal as f64 / f64::from(months)
    } else {
        principal as f64 * rate / -(-f64::from(months) * rate.ln_1p()).exp_m1()
    }
    .round() as i64;
    let mut balance = principal;
    let mut total_interest = 0i64;
    let mut total_payment = 0i64;
    let mut rows = vec![vec![
        "回".into(),
        "返済前残高".into(),
        "利息".into(),
        "元金返済".into(),
        "返済額".into(),
        "返済後残高".into(),
    ]];
    for month in 1..=months {
        checkpoint(cancelled)?;
        let interest = (balance as f64 * rate).round() as i64;
        let repaid = if month == months {
            balance
        } else if equal_principal {
            principal / i64::from(months)
        } else {
            (payment - interest).max(0)
        }
        .min(balance);
        let paid = repaid + interest;
        rows.push(vec![
            month.to_string(),
            units(balance, decimals),
            units(interest, decimals),
            units(repaid, decimals),
            units(paid, decimals),
            units(balance - repaid, decimals),
        ]);
        balance -= repaid;
        total_interest += interest;
        total_payment += paid;
        if balance == 0 {
            break;
        }
    }
    Output::csv(
        rows,
        format!(
            "返済総額 {}・利息合計 {}。毎月の利息を小数{decimals}桁で四捨五入、最終回調整。税金・手数料なし",
            units(total_payment, decimals),
            units(total_interest, decimals)
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn zero_rate_and_period_boundaries() {
        let mut p = crate::Tool::Compound.defaults();
        p.insert("rate".into(), "0".into());
        let out = compound(&p).unwrap();
        assert!(out.text.contains("10,2200000,0,2200000"));
        p.insert("rate".into(), "NaN".into());
        assert!(compound(&p).is_err());
        let mut p = crate::Tool::Loan.defaults();
        p.insert("rate".into(), "0".into());
        p.insert("principal".into(), "100".into());
        p.insert("months".into(), "3".into());
        let out = loan(&p, &|| false).unwrap();
        assert!(out.text.contains("3,34,0,34,34,0"));
    }
    #[test]
    fn pmt_reference_and_exact_principal_conservation() {
        let mut p = crate::Tool::Loan.defaults();
        for (k, v) in [
            ("principal", "10000"),
            ("rate", "8"),
            ("months", "10"),
            ("decimals", "2"),
        ] {
            p.insert(k.into(), v.into());
        }
        for method in ["元利均等", "元金均等"] {
            p.insert("method".into(), method.into());
            let out = loan(&p, &|| false).unwrap();
            let records: Vec<_> = csv::Reader::from_reader(out.text.as_bytes())
                .records()
                .map(|r| r.unwrap())
                .collect();
            assert_eq!(records.len(), 10);
            assert_eq!(&records[9][5], "0.00");
            let paid: i64 = records
                .iter()
                .map(|r| principal_units(&r[3], 2).unwrap())
                .sum();
            assert_eq!(paid, 1_000_000);
            if method == "元利均等" {
                assert_eq!(&records[0][4], "1037.03");
            }
        }
    }
}
