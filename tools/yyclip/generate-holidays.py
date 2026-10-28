"""Generate the bundled 2026-2031 calendar; see apps/yyclip/data/README.md."""
import argparse
import csv
import datetime as dt
import io
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUTPUT = ROOT / "apps/yyclip/data/japan-holidays-2026-2031.csv"
# NAOJ's published predictions, not a declaration of future statutory holidays.
EQUINOXES = {2026: (20, 23), 2027: (21, 23), 2028: (20, 22),
             2029: (20, 23), 2030: (20, 23), 2031: (21, 23)}


def holidays(year):
    result = {}
    for month, day, name in [(1, 1, "元日"), (2, 11, "建国記念の日"), (2, 23, "天皇誕生日"),
                             (4, 29, "昭和の日"), (5, 3, "憲法記念日"), (5, 4, "みどりの日"),
                             (5, 5, "こどもの日"), (8, 11, "山の日"), (11, 3, "文化の日"),
                             (11, 23, "勤労感謝の日"), (3, EQUINOXES[year][0], "春分の日"),
                             (9, EQUINOXES[year][1], "秋分の日")]:
        result[dt.date(year, month, day)] = name
    for month, nth, name in [(1, 2, "成人の日"), (7, 3, "海の日"),
                              (9, 3, "敬老の日"), (10, 2, "スポーツの日")]:
        first = dt.date(year, month, 1)
        result[first + dt.timedelta(days=(-first.weekday()) % 7 + (nth - 1) * 7)] = name
    national = set(result)
    for date in sorted(national):
        middle = date + dt.timedelta(days=1)
        if middle not in national and middle + dt.timedelta(days=1) in national:
            result[middle] = "国民の休日"
    for date in sorted(national):
        if date.weekday() == 6:
            substitute = date + dt.timedelta(days=1)
            while substitute in national:
                substitute += dt.timedelta(days=1)
            result[substitute] = "振替休日"
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--verify-cabinet", type=Path)
    args = parser.parse_args()
    dates = {date: name for year in EQUINOXES for date, name in holidays(year).items()}
    if args.verify_cabinet:
        official = set()
        with args.verify_cabinet.open(encoding="cp932", newline="") as stream:
            for row in list(csv.reader(stream))[1:]:
                date = dt.datetime.strptime(row[0], "%Y/%m/%d").date()
                if 2026 <= date.year <= 2027:
                    official.add(date)
        generated = {date for date in dates if 2026 <= date.year <= 2027}
        assert official and official == generated, (official - generated, generated - official)
        print(f"Verified {len(official)} dates against Cabinet Office CSV")
    buffer = io.StringIO(newline="")
    writer = csv.writer(buffer, lineterminator="\n")
    writer.writerow(["日付", "名称"])
    writer.writerows((date.isoformat(), dates[date]) for date in sorted(dates))
    data = buffer.getvalue().encode("utf-8-sig")
    if args.check:
        assert OUTPUT.read_bytes() == data, "Bundled calendar is stale"
    else:
        OUTPUT.parent.mkdir(parents=True, exist_ok=True)
        OUTPUT.write_bytes(data)
    print(f"{len(dates)} holidays: {OUTPUT}")


if __name__ == "__main__":
    main()
