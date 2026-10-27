//! GFM の表の構造編集。表の外と、行操作で変更しない行は原文のまま残す。
use pulldown_cmark::{Event, Options, Parser, Tag};
use std::ops::Range;

#[derive(Clone, Copy)]
pub enum Operation {
    ColumnBefore,
    ColumnAfter,
    DeleteColumn,
    RowBefore,
    RowAfter,
    DeleteRow,
}

impl Operation {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "column-before" => Self::ColumnBefore,
            "column-after" => Self::ColumnAfter,
            "delete-column" => Self::DeleteColumn,
            "row-before" => Self::RowBefore,
            "row-after" => Self::RowAfter,
            "delete-row" => Self::DeleteRow,
            _ => return None,
        })
    }
}

pub struct Edited {
    pub source: String,
    pub caret: usize,
}
pub struct Context {
    pub row: usize,
    pub column: usize,
    pub rows: usize,
    pub columns: usize,
}

struct Row {
    start: usize,
    end: usize,
    prefix: String,
    cells: Vec<String>,
    suffix: String,
    eol: String,
}

impl Row {
    fn parse(text: &str, start: usize) -> Self {
        let content = text.trim_end_matches(['\r', '\n']);
        let eol = text[content.len()..].to_owned();
        let mut pipes = Vec::new();
        let mut slashes = 0;
        for (i, b) in content.bytes().enumerate() {
            if b == b'|' && slashes % 2 == 0 {
                pipes.push(i);
            }
            if b == b'\\' {
                slashes += 1;
            } else {
                slashes = 0;
            }
        }
        let leading = pipes
            .first()
            .copied()
            .filter(|i| content[..*i].trim().is_empty());
        let trailing = pipes
            .last()
            .copied()
            .filter(|i| Some(*i) != leading && content[*i + 1..].trim().is_empty());
        let first = leading.map_or(0, |i| i + 1);
        let last = trailing.unwrap_or(content.len());
        let mut cells = Vec::new();
        let mut pos = first;
        for pipe in pipes.into_iter().filter(|i| *i >= first && *i < last) {
            cells.push(content[pos..pipe].to_owned());
            pos = pipe + 1;
        }
        cells.push(content[pos..last].to_owned());
        Self {
            start,
            end: start + text.len(),
            prefix: content[..first].to_owned(),
            cells,
            suffix: content[last..].to_owned(),
            eol,
        }
    }
    fn text(&self) -> String {
        format!(
            "{}{}{}{}",
            self.prefix,
            self.cells.join("|"),
            self.suffix,
            self.eol
        )
    }
    fn cell_start(&self, column: usize) -> usize {
        self.start
            + self.prefix.len()
            + self
                .cells
                .iter()
                .take(column)
                .map(|s| s.len() + 1)
                .sum::<usize>()
    }
    fn column(&self, caret: usize, columns: usize) -> usize {
        (0..self.cells.len())
            .find(|c| caret <= self.cell_start(*c) + self.cells[*c].len())
            .unwrap_or(self.cells.len() - 1)
            .min(columns - 1)
    }
}

struct Table {
    range: Range<usize>,
    rows: Vec<Row>,
    columns: usize,
    row: usize,
    column: usize,
}

fn table_at(source: &str, caret: usize) -> Option<Table> {
    let mut depth = 0;
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_MATH;
    for (event, range) in Parser::new_ext(source, options).into_offset_iter() {
        if let Event::Start(Tag::Table(ref alignments)) = event
            && depth == 0
            && range.contains(&caret)
        {
            let rows: Vec<Row> = source[range.clone()]
                .split_inclusive('\n')
                .scan(range.start, |offset, line| {
                    let row = Row::parse(line, *offset);
                    *offset += line.len();
                    Some(row)
                })
                .collect();
            if rows.len() < 2 || alignments.is_empty() {
                return None;
            }
            let raw_row = rows
                .iter()
                .position(|r| caret < r.end)
                .unwrap_or(rows.len() - 1);
            let column = rows[raw_row].column(caret, alignments.len());
            return Some(Table {
                range,
                rows,
                columns: alignments.len(),
                row: if raw_row == 1 { 0 } else { raw_row },
                column,
            });
        }
        match event {
            Event::Start(_) => depth += 1,
            Event::End(_) => depth -= 1,
            _ => {}
        }
    }
    None
}

pub fn context(source: &str, caret: usize) -> Option<Context> {
    let table = table_at(source, caret)?;
    Some(Context {
        row: if table.row == 0 { 0 } else { table.row - 1 },
        column: table.column,
        rows: table.rows.len() - 1,
        columns: table.columns,
    })
}

pub fn edit(source: &str, caret: usize, operation: Operation) -> Result<Edited, &'static str> {
    let mut table=table_at(source,caret).ok_or("編集する表のセルをクリックするか、表の原文にカーソルを置いてください（入れ子の表は原文で編集してください）。")?;
    let mut row = table.row;
    let mut column = table.column;
    let default_eol = table
        .rows
        .iter()
        .find(|r| !r.eol.is_empty())
        .map(|r| r.eol.clone())
        .unwrap_or_else(|| "\n".into());
    match operation {
        Operation::ColumnBefore | Operation::ColumnAfter | Operation::DeleteColumn => {
            if matches!(operation, Operation::DeleteColumn) && table.columns == 1 {
                return Err("最後の1列は削除できません。表全体の削除は原文で行ってください。");
            }
            let index = column + usize::from(matches!(operation, Operation::ColumnAfter));
            let delimiter = table.rows[1]
                .cells
                .get(column)
                .cloned()
                .unwrap_or_else(|| " --- ".into());
            for (n, r) in table.rows.iter_mut().enumerate() {
                r.cells
                    .resize_with(r.cells.len().max(table.columns), || "  ".into());
                if matches!(operation, Operation::DeleteColumn) {
                    r.cells.remove(column);
                } else {
                    r.cells.insert(
                        index,
                        if n == 1 {
                            delimiter.clone()
                        } else {
                            "  ".into()
                        },
                    );
                }
                if r.prefix.is_empty() && r.cells.first().is_some_and(|c| c.trim().is_empty()) {
                    r.prefix = "|".into();
                }
                if r.suffix.is_empty() && r.cells.last().is_some_and(|c| c.trim().is_empty()) {
                    r.suffix = "|".into();
                }
            }
            if matches!(operation, Operation::DeleteColumn) {
                column = column.min(table.columns - 2);
            } else {
                column = index;
            }
        }
        Operation::RowBefore | Operation::RowAfter => {
            if row == 0 && matches!(operation, Operation::RowBefore) {
                return Err("見出し行の上には追加できません。下に行を追加してください。");
            }
            let index = if row == 0 {
                2
            } else {
                row + usize::from(matches!(operation, Operation::RowAfter))
            };
            let template = table
                .rows
                .get(if row == 0 { 2 } else { row })
                .unwrap_or(&table.rows[0]);
            let new_row = Row {
                start: 0,
                end: 0,
                prefix: if template.prefix.is_empty() {
                    "|".into()
                } else {
                    template.prefix.clone()
                },
                cells: vec!["  ".into(); table.columns],
                suffix: if template.suffix.is_empty() {
                    "|".into()
                } else {
                    template.suffix.clone()
                },
                eol: if index == table.rows.len()
                    && table.rows.last().is_some_and(|r| r.eol.is_empty())
                {
                    String::new()
                } else {
                    default_eol.clone()
                },
            };
            if index > 0 && table.rows[index - 1].eol.is_empty() {
                table.rows[index - 1].eol = default_eol;
            }
            table.rows.insert(index, new_row);
            row = index;
        }
        Operation::DeleteRow => {
            if row == 0 {
                return Err("見出し行は削除できません。本文の行を選んでください。");
            }
            let deleted = table.rows.remove(row);
            if row == table.rows.len() && deleted.eol.is_empty() {
                table.rows.last_mut().unwrap().eol.clear();
            }
            row = if table.rows.len() == 2 {
                0
            } else {
                row.min(table.rows.len() - 1)
            };
        }
    }
    let mut text = String::new();
    for r in &mut table.rows {
        r.start = table.range.start + text.len();
        text += &r.text();
    }
    let caret = table.rows[row].cell_start(column.min(table.rows[row].cells.len() - 1));
    let mut updated = source.to_owned();
    updated.replace_range(table.range, text.as_str());
    Ok(Edited {
        source: updated,
        caret,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn columns_preserve_alignment_escaped_pipes_and_surrounding_source() {
        let src =
            "# 前\r\n\r\n| 日本語 | B |\r\n| :--- | ---: |\r\n| a\\|b | `c\\|d` |\r\n\r\n後ろ\r\n";
        let pos = src.find("日本語").unwrap();
        let added = edit(src, pos, Operation::ColumnAfter).unwrap();
        assert!(
            added
                .source
                .contains("| 日本語 |  | B |\r\n| :--- | :--- | ---: |")
        );
        assert!(added.source.contains("| a\\|b |  | `c\\|d` |"));
        let removed = edit(&added.source, added.caret, Operation::DeleteColumn).unwrap();
        assert_eq!(removed.source, src);
        let ctx = context(src, src.find("`c").unwrap()).unwrap();
        assert_eq!((ctx.row, ctx.column, ctx.rows, ctx.columns), (1, 1, 2, 2));
    }
    #[test]
    fn rows_handle_header_only_tables_and_no_final_newline() {
        let src = "A | B\n--- | ---";
        assert!(edit(src, 0, Operation::DeleteRow).is_err());
        assert!(edit(src, 0, Operation::RowBefore).is_err());
        let added = edit(src, 0, Operation::RowAfter).unwrap();
        assert_eq!(added.source, "A | B\n--- | ---\n|  |  |");
        let removed = edit(&added.source, added.caret, Operation::DeleteRow).unwrap();
        assert_eq!(removed.source, src);
        assert!(edit("| A |\n| --- |\n", 2, Operation::DeleteColumn).is_err());
        assert!(edit("```\nA | B\n--- | ---\n```", 7, Operation::RowAfter).is_err());
        assert!(edit("> | A | B |\n> | --- | --- |\n", 4, Operation::ColumnAfter).is_err());
        let plain = "A|B\n---|---\nx|y\n";
        for (caret, operation) in [(0, Operation::ColumnBefore), (2, Operation::ColumnAfter)] {
            let added = edit(plain, caret, operation).unwrap();
            assert_eq!(context(&added.source, added.caret).unwrap().columns, 3);
        }
    }
    #[test]
    fn inserts_before_and_after_body_rows_without_rewriting_other_rows() {
        let src = "| A | B |\n| --- | --- |\n| one | x |\n| two | y |\n";
        let before = edit(src, src.find("two").unwrap(), Operation::RowBefore).unwrap();
        assert_eq!(
            before.source,
            "| A | B |\n| --- | --- |\n| one | x |\n|  |  |\n| two | y |\n"
        );
        let after = edit(src, src.find("one").unwrap(), Operation::RowAfter).unwrap();
        assert_eq!(after.source, before.source);
        let removed = edit(&before.source, before.caret, Operation::DeleteRow).unwrap();
        assert_eq!(removed.source, src);
    }
}
