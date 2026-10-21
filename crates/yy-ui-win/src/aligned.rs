//! Optional virtual source-row mapping for read-only comparison panes.
use crate::render::RowDecoration;

#[derive(Clone, Copy)]
pub struct DisplayRow {
    /// Original document byte offset; None is a display-only placeholder.
    pub source: Option<u64>,
    pub decoration: RowDecoration,
}
pub struct AlignedView {
    pub rows: Vec<DisplayRow>,
    pub top: usize,
    /// Different-height synchronized panes may scroll the last row to the top.
    pub pad_bottom: bool,
    source_index: Vec<(u64, usize)>,
}
impl AlignedView {
    pub fn new(rows: Vec<DisplayRow>) -> Self {
        let source_index = rows
            .iter()
            .enumerate()
            .filter_map(|(i, r)| r.source.map(|s| (s, i)))
            .collect::<Vec<_>>();
        debug_assert!(source_index.windows(2).all(|p| p[0].0 < p[1].0));
        Self {
            rows,
            top: 0,
            pad_bottom: false,
            source_index,
        }
    }
    pub fn row_for_offset(&self, offset: u64) -> usize {
        let i = self
            .source_index
            .partition_point(|(at, _)| *at <= offset)
            .saturating_sub(1);
        self.source_index.get(i).map_or(0, |(_, row)| *row)
    }
    pub fn source_near_row(&self, row: usize) -> Option<u64> {
        let i = self.source_index.partition_point(|(_, at)| *at < row);
        self.source_index
            .get(i)
            .or_else(|| self.source_index.last())
            .map(|(at, _)| *at)
    }
    pub fn scroll(&mut self, delta: i64, page: usize) {
        self.top = self
            .top
            .saturating_add_signed(delta.clamp(isize::MIN as i64, isize::MAX as i64) as isize)
            .min(self.last_top(page));
    }
    pub fn last_top(&self, page: usize) -> usize {
        self.rows
            .len()
            .saturating_sub(if self.pad_bottom { 1 } else { page.max(1) })
    }
    pub fn reveal(&mut self, offset: u64, page: usize) {
        let row = self.row_for_offset(offset);
        if row < self.top {
            self.top = row;
        } else if row >= self.top + page.max(1) {
            self.top = row.saturating_sub(page.max(1) - 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn row(source: Option<u64>) -> DisplayRow {
        DisplayRow {
            source,
            decoration: RowDecoration {
                line_number: source.map(|n| n / 10 + 1),
                background: None,
            },
        }
    }
    #[test]
    fn placeholders_do_not_change_source_coordinates() {
        let mut view = AlignedView::new(vec![
            row(Some(0)),
            row(None),
            row(Some(10)),
            row(None),
            row(Some(20)),
        ]);
        assert_eq!(view.row_for_offset(15), 2);
        assert_eq!(view.source_near_row(1), Some(10));
        view.reveal(20, 2);
        assert_eq!(view.top, 3);
        view.scroll(-2, 2);
        assert_eq!(view.top, 1);
        view.scroll(i64::MAX, 2);
        assert_eq!(view.top, 3);
        view.scroll(i64::MIN, 2);
        assert_eq!(view.top, 0);
        view.pad_bottom = true;
        view.scroll(i64::MAX, 100);
        assert_eq!(view.top, 4);
        assert_eq!(view.last_top(200), 4);
    }
}
