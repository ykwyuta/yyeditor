use super::*;

fn f(code: &str, v: f64) -> String {
    let r = Format::parse(code).format(FmtValue::Number(v), DateSystem::D1900);
    if r.overflow { "#".into() } else { r.text }
}

fn ft(code: &str, s: &str) -> String {
    Format::parse(code)
        .format(FmtValue::Text(s), DateSystem::D1900)
        .text
}

#[test]
fn numbers() {
    for (code, v, want) in [
        ("0", 1234.5, "1235"),
        ("0", -1234.5, "-1235"),
        ("0", 0.4, "0"),
        ("0.00", 3.14259, "3.14"),
        ("0.00", 2.675, "2.68"),
        ("0.00", 1.005, "1.01"),
        ("#,##0", 1234567.0, "1,234,567"),
        ("#,##0", -1234.5, "-1,235"),
        ("#,##0", 0.0, "0"),
        ("#,##0", 999.6, "1,000"),
        ("#,##0.00", 1234.567, "1,234.57"),
        ("#,##0.00", 0.5, "0.50"),
        ("#.##", 0.5, ".5"),
        ("#.##", 2.0, "2."),
        ("0.0#", 1.5, "1.5"),
        ("0.0#", 1.256, "1.26"),
        ("00000", 123.0, "00123"),
        ("0%", 0.153, "15%"),
        ("0.00%", 0.15345, "15.35%"),
        ("#,##0,", 1234567.0, "1,235"),
        ("0.0,,", 1234567.0, "1.2"),
        ("0.00E+00", 12345.0, "1.23E+04"),
        ("0.00E+00", 0.000123, "1.23E-04"),
        ("0.00E+00", 0.0, "0.00E+00"),
        ("0.0E+0", 99999.0, "1.0E+5"),
        ("##0.0E+0", 12345.0, "12.3E+3"),
        ("\"¥\"#,##0", 1500.0, "¥1,500"),
        ("[$¥-411]#,##0", 1500.0, "¥1,500"),
        ("#,##0\"円\"", 2500.0, "2,500円"),
        ("0_);(0)", 5.0, "5 "),
        ("0_);(0)", -5.0, "(5)"),
        ("0;-0;\"ゼロ\"", 0.0, "ゼロ"),
        ("0;-0;\"ゼロ\"", -5.0, "-5"),
        ("0;\"マイナス\"0", -5.0, "マイナス5"),
        ("General", 0.1 + 0.2, "0.3"),
        ("標準", 1234.5, "1234.5"),
        ("# ?/?", 1.5, "1 1/2"),
        ("# ??/16", 1.3, "1  5/16"),
        ("?/?", 0.25, "1/4"),
        ("0.0", 9.96, "10.0"),
        ("0", 123456789012345.0, "123456789012345"),
    ] {
        assert_eq!(f(code, v), want, "{code} {v}");
    }
}

#[test]
fn sections_conditions_colors() {
    let r = Format::parse("[Red]0;[Blue]-0").format(FmtValue::Number(-3.0), DateSystem::D1900);
    assert_eq!((r.text.as_str(), r.color), ("-3", Some(FmtColor::Blue)));
    let r = Format::parse("[赤]#,##0").format(FmtValue::Number(5.0), DateSystem::D1900);
    assert_eq!(r.color, Some(FmtColor::Red));
    let r = Format::parse("[Color10]0").format(FmtValue::Number(5.0), DateSystem::D1900);
    assert_eq!(r.color.map(FmtColor::rgb), Some((0, 128, 0)));
    let c = "[>=100]\"大\";[<0]\"負\";\"小\"";
    assert_eq!(f(c, 150.0), "大");
    assert_eq!(f(c, -1.0), "負");
    assert_eq!(f(c, 5.0), "小");
    assert_eq!(ft("@\"様\"", "田中"), "田中様");
    assert_eq!(ft("0;0;0;\"文字:\"@", "x"), "文字:x");
    assert_eq!(ft("0.00", "そのまま"), "そのまま");
    let r = Format::parse("* #,##0").format(FmtValue::Number(5.0), DateSystem::D1900);
    assert_eq!(r.fill, Some((0, ' ')));
}

#[test]
fn dates_and_times() {
    let d = 46302.75; // 2026/10/7 18:00（水）
    for (code, want) in [
        ("yyyy/m/d", "2026/10/7"),
        ("yyyy/mm/dd hh:mm:ss", "2026/10/07 18:00:00"),
        ("m/d/yy", "10/7/26"),
        ("mmm d, yyyy", "Oct 7, 2026"),
        ("mmmm", "October"),
        ("mmmmm", "O"),
        ("dddd", "Wednesday"),
        ("ddd", "Wed"),
        ("aaa", "水"),
        ("aaaa", "水曜日"),
        ("h:mm AM/PM", "6:00 PM"),
        ("h:mm 午前/午後", "6:00 午後"),
        ("ggge年m月d日", "令和8年10月7日"),
        ("ge.m.d", "R8.10.7"),
        ("gge", "令8"),
        ("yyyy年m月d日(aaa)", "2026年10月7日(水)"),
    ] {
        assert_eq!(f(code, d), want, "{code}");
    }
    assert_eq!(f("[h]:mm", 1.5), "36:00");
    assert_eq!(f("mm:ss", 90.0 / 86_400.0), "01:30");
    assert_eq!(f("[mm]:ss", 3700.0 / 86_400.0), "61:40");
    assert_eq!(f("h:mm:ss.000", 0.5 + 1.234 / 86_400.0), "12:00:01.234");
    assert_eq!(f("h:mm:ss", 0.5 + 0.6 / 86_400.0), "12:00:01");
    assert_eq!(f("ggge年", 32_000.0), "昭和62年");
    assert_eq!(f("ggge年", 33_000.0), "平成2年");
    assert_eq!(f("yyyy/m/d", -1.0), "#");
    assert!(Format::parse("yyyy/m/d").is_date());
    assert!(!Format::parse("#,##0").is_date());
    assert!(Format::parse("[h]:mm").is_date());
}

#[test]
fn general_fits_width() {
    assert_eq!(general_fit(1234.5, 11).as_deref(), Some("1234.5"));
    assert_eq!(general_fit(1_234_567.891, 8).as_deref(), Some("1234568"));
    assert_eq!(general_fit(0.123_456_789, 6).as_deref(), Some("0.1235"));
    assert_eq!(
        general_fit(123_456_789_012.0, 10).as_deref(),
        Some("1.2346E+11")
    );
    assert_eq!(general_fit(123_456_789_012.0, 3), None);
}
