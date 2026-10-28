//! Native presentation and responsive layout; no WebView or runtime dependency.
use super::*;

const BACKGROUND: COLORREF = COLORREF(0x00fcf9f6);
const INK: COLORREF = COLORREF(0x00372b20);
const MUTED: COLORREF = COLORREF(0x00766658);
const ACCENT: COLORREF = COLORREF(0x00c45325);

fn extent(hwnd: HWND, content: &str, width: i32, wrap: bool) -> SIZE {
    unsafe {
        let dc = GetDC(Some(hwnd));
        let font = HFONT(SendMessageW(hwnd, WM_GETFONT, None, None).0 as *mut _);
        let old = SelectObject(dc, font.into());
        let mut chars: Vec<_> = content.encode_utf16().collect();
        let mut rect = RECT {
            right: width.max(1),
            ..Default::default()
        };
        DrawTextW(
            dc,
            &mut chars,
            &mut rect,
            DT_CALCRECT | DT_NOPREFIX | if wrap { DT_WORDBREAK } else { DT_SINGLELINE },
        );
        SelectObject(dc, old);
        ReleaseDC(Some(hwnd), dc);
        SIZE {
            cx: rect.right,
            cy: rect.bottom,
        }
    }
}

impl State {
    fn display_dpi(&self) -> u32 {
        #[cfg(test)]
        if let Some(dpi) = self.test_dpi {
            return dpi;
        }
        unsafe { GetDpiForWindow(self.hwnd) }.max(96)
    }
    pub(super) fn scale(&self, n: i32) -> i32 {
        n * self.display_dpi() as i32 * self.zoom as i32 / 9600
    }

    pub(super) fn refresh_fonts(&mut self) {
        let dpi = self.display_dpi() * self.zoom / 100;
        let font = ui_font(dpi, false);
        let mono = ui_font(dpi, true);
        unsafe {
            let mut lf = LOGFONTW {
                lfHeight: -(22 * dpi as i32 / 96),
                lfWeight: 600,
                ..Default::default()
            };
            let name: Vec<_> = "Segoe UI".encode_utf16().collect();
            lf.lfFaceName[..name.len()].copy_from_slice(&name);
            let title = CreateFontIndirectW(&lf);
            let mut controls = vec![
                self.select_label,
                self.select,
                self.description,
                self.input_label,
                self.output_label,
                self.status,
            ];
            controls.extend(self.buttons.iter().map(|b| b.1));
            controls.extend(self.zoom_buttons);
            for f in &self.fields {
                controls.extend([f.label, f.control]);
            }
            for h in controls {
                SendMessageW(
                    h,
                    WM_SETFONT,
                    Some(WPARAM(font.0 as usize)),
                    Some(LPARAM(1)),
                );
            }
            SendMessageW(
                self.heading,
                WM_SETFONT,
                Some(WPARAM(title.0 as usize)),
                Some(LPARAM(1)),
            );
            for h in [self.input, self.output] {
                SendMessageW(
                    h,
                    WM_SETFONT,
                    Some(WPARAM(mono.0 as usize)),
                    Some(LPARAM(1)),
                );
            }
            for old in self.fonts.drain(..) {
                let _ = DeleteObject(old.into());
            }
            self.fonts = vec![font, mono, title];
            self.font = font;
            self.mono = mono;
            self.title_font = title;
        }
        set_text(self.zoom_buttons[1], &format!("{}%", self.zoom));
        self.layout();
    }

    pub(super) fn zoom_to(&mut self, percent: u32) {
        let focus = unsafe { GetFocus() };
        self.zoom = percent.clamp(75, 200);
        self.scroll.set(0);
        self.refresh_fonts();
        if unsafe { IsChild(self.hwnd, focus) }.as_bool() {
            self.reveal(focus);
        }
    }

    pub(super) fn button_visible(&self, id: u16) -> bool {
        match id {
            EXAMPLE | OPEN | SAVE_INPUT => uses_input(self.tool),
            CANCEL => self.running.is_some(),
            _ => true,
        }
    }

    pub(super) fn arrange(&self) {
        unsafe {
            let mut client = RECT::default();
            if GetClientRect(self.hwnd, &mut client).is_err() {
                return;
            }
            let d = |n| self.scale(n);
            let pad = d(24).min(client.right / 12).max(8);
            let gap = d(12);
            let width = (client.right - 2 * pad).max(1);
            let row = d(38);
            let label_h = d(24);
            let mut positions = Vec::new();
            let mut put = |h, x, y, w, height| positions.push((h, x, y, w, height));
            let mut y = pad;
            put(self.select_label, pad, y, width, label_h);
            let mut zoom_w = [d(54), d(72), d(54)];
            let desired = zoom_w.iter().sum::<i32>();
            let available = (width - d(12)).max(1);
            if desired > available {
                for w in &mut zoom_w {
                    *w = *w * available / desired;
                }
            }
            let total_zoom = zoom_w.iter().sum::<i32>() + d(12);
            if width < total_zoom + d(140) {
                y += label_h + gap;
            }
            let mut x = pad + width - total_zoom;
            for (i, h) in self.zoom_buttons.iter().enumerate() {
                put(*h, x, y, zoom_w[i], row);
                x += zoom_w[i] + d(6);
            }
            y += row + gap;
            let title_h = extent(self.heading, &text(self.heading), width, true)
                .cy
                .max(d(30))
                + d(4);
            put(self.heading, pad, y, width, title_h);
            y += title_h + gap;
            SendMessageW(
                self.select,
                CB_SETITEMHEIGHT,
                Some(WPARAM(usize::MAX)),
                Some(LPARAM(d(30) as isize)),
            );
            SendMessageW(
                self.select,
                CB_SETITEMHEIGHT,
                Some(WPARAM(0)),
                Some(LPARAM(d(28) as isize)),
            );
            put(self.select, pad, y, width, d(360));
            y += d(38) + gap;
            let description_h =
                extent(self.description, &text(self.description), width, true).cy + d(6);
            put(self.description, pad, y, width, description_h);
            y += description_h + d(20);

            let columns = if width >= d(620) { 2 } else { 1 };
            let col = (width - (columns - 1) * gap) / columns;
            for (i, field) in self.fields.iter().enumerate() {
                let x = pad + i as i32 % columns * (col + gap);
                let fy = y + i as i32 / columns * (label_h + row + gap);
                put(field.label, x, fy, col, label_h);
                if !field.definition.choices.is_empty() {
                    SendMessageW(
                        field.control,
                        CB_SETITEMHEIGHT,
                        Some(WPARAM(usize::MAX)),
                        Some(LPARAM(d(30) as isize)),
                    );
                    SendMessageW(
                        field.control,
                        CB_SETITEMHEIGHT,
                        Some(WPARAM(0)),
                        Some(LPARAM(d(28) as isize)),
                    );
                    let widest = field
                        .definition
                        .choices
                        .iter()
                        .map(|s| extent(field.control, s, 10000, false).cx)
                        .max()
                        .unwrap_or(0)
                        + d(36);
                    SendMessageW(
                        field.control,
                        CB_SETDROPPEDWIDTH,
                        Some(WPARAM(widest.max(col) as usize)),
                        None,
                    );
                }
                put(
                    field.control,
                    x,
                    fy + label_h,
                    col,
                    if field.definition.choices.is_empty() {
                        row
                    } else {
                        d(360)
                    },
                );
            }
            y += (self.fields.len() as i32 + columns - 1) / columns * (label_h + row + gap);
            if !self.fields.is_empty() {
                y += d(8);
            }
            x = pad;
            for (id, button, _) in &self.buttons {
                let visible = self.button_visible(*id);
                show(*button, visible);
                if !visible {
                    continue;
                }
                let w = (extent(*button, &text(*button), width, false).cx + d(32))
                    .max(d(if *id == RUN { 112 } else { 64 }))
                    .min(width);
                if x > pad && x + w > pad + width {
                    x = pad;
                    y += row + gap;
                }
                put(*button, x, y, w, row);
                x += w + gap;
            }
            y += row + d(24);
            let input = uses_input(self.tool);
            let qr = self.result.as_ref().is_some_and(|r| r.qr.is_some());
            show(self.input_label, input);
            show(self.input, input);
            show(self.output, !qr);
            show(self.qr_view, qr);
            let output = if qr { self.qr_view } else { self.output };
            let status_h = extent(self.status, &text(self.status), width, true)
                .cy
                .max(d(22))
                + d(8);
            let side_by_side = input && width >= d(850);
            let count = if input && !side_by_side { 2 } else { 1 };
            let area = ((client.bottom - y - pad - status_h - d(16)) / count - label_h - gap)
                .max(d(if count == 2 { 160 } else { 220 }))
                .min((client.bottom - d(28)).max(d(48)));
            let pane_width = if side_by_side {
                (width - gap) / 2
            } else {
                width
            };
            if input {
                put(self.input_label, pad, y, pane_width, label_h);
                put(self.input, pad, y + label_h, pane_width, area);
                if !side_by_side {
                    y += label_h + area + gap;
                }
            }
            let output_x = if side_by_side {
                pad + pane_width + gap
            } else {
                pad
            };
            put(self.output_label, output_x, y, pane_width, label_h);
            put(output, output_x, y + label_h, pane_width, area);
            y += label_h + area + d(16);
            put(self.status, pad, y, width, status_h);
            let total = y + status_h + pad;
            self.content_height.set(total);
            let offset = self.scroll.get().clamp(0, (total - client.bottom).max(0));
            self.scroll.set(offset);
            let info = SCROLLINFO {
                cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                fMask: SIF_RANGE | SIF_PAGE | SIF_POS | SIF_DISABLENOSCROLL,
                nMin: 0,
                nMax: total - 1,
                nPage: client.bottom.max(1) as u32,
                nPos: offset,
                ..Default::default()
            };
            SetScrollInfo(self.hwnd, SB_VERT, &info, true);
            for (h, x, y, w, height) in positions {
                let _ = MoveWindow(h, x, y - offset, w.max(1), height.max(1), false);
            }
            let _ = RedrawWindow(
                Some(self.hwnd),
                None,
                None,
                RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN,
            );
        }
    }

    pub(super) fn scroll_by(&self, delta: i32) {
        self.scroll.set(self.scroll.get().saturating_add(delta));
        self.layout();
    }

    pub(super) fn reveal(&self, control: HWND) {
        unsafe {
            let mut rect = RECT::default();
            let mut origin = POINT::default();
            let mut client = RECT::default();
            if GetWindowRect(control, &mut rect).is_err()
                || !ClientToScreen(self.hwnd, &mut origin).as_bool()
                || GetClientRect(self.hwnd, &mut client).is_err()
            {
                return;
            }
            let top = rect.top - origin.y;
            let bottom = rect.bottom - origin.y;
            let delta = if top < 0 {
                top - self.scale(12)
            } else if bottom > client.bottom {
                bottom - client.bottom + self.scale(12)
            } else {
                0
            };
            if delta != 0 {
                self.scroll_by(delta);
            }
        }
    }
}

pub(super) fn color(hwnd: HWND, message: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    state(|s| unsafe {
        let dc = HDC(wp.0 as *mut _);
        let control = HWND(lp.0 as *mut _);
        let editor =
            message == WM_CTLCOLOREDIT || control == s.output || message == WM_CTLCOLORLISTBOX;
        let _ = SetBkMode(dc, TRANSPARENT);
        SetTextColor(
            dc,
            if control == s.heading || editor {
                INK
            } else if control == s.select_label {
                ACCENT
            } else {
                MUTED
            },
        );
        SetBkColor(
            dc,
            if editor {
                COLORREF(0x00ffffff)
            } else {
                BACKGROUND
            },
        );
        LRESULT(if editor { s.paper.0 } else { s.background.0 } as isize)
    })
    .unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, message, wp, lp) })
}

pub(super) fn draw_button(item: &DRAWITEMSTRUCT) {
    unsafe {
        let disabled = item.itemState.0 & ODS_DISABLED.0 != 0;
        let selected = item.itemState.0 & ODS_SELECTED.0 != 0;
        let primary = item.CtlID == u32::from(RUN);
        let fill = if disabled {
            COLORREF(0x00f2efeb)
        } else if primary {
            if selected {
                COLORREF(0x00963e1c)
            } else {
                ACCENT
            }
        } else if selected {
            COLORREF(0x00eee6de)
        } else {
            COLORREF(0x00ffffff)
        };
        let brush = CreateSolidBrush(fill);
        let pen = CreatePen(
            PS_SOLID,
            1,
            if primary { fill } else { COLORREF(0x00dfd7ce) },
        );
        let old_b = SelectObject(item.hDC, brush.into());
        let old_p = SelectObject(item.hDC, pen.into());
        let _ = RoundRect(
            item.hDC,
            item.rcItem.left,
            item.rcItem.top,
            item.rcItem.right,
            item.rcItem.bottom,
            10,
            10,
        );
        SelectObject(item.hDC, old_b);
        SelectObject(item.hDC, old_p);
        let _ = DeleteObject(brush.into());
        let _ = DeleteObject(pen.into());
        let font = HFONT(SendMessageW(item.hwndItem, WM_GETFONT, None, None).0 as *mut _);
        let old = SelectObject(item.hDC, font.into());
        let _ = SetBkMode(item.hDC, TRANSPARENT);
        SetTextColor(
            item.hDC,
            if disabled {
                COLORREF(0x00998d81)
            } else if primary {
                COLORREF(0x00ffffff)
            } else {
                INK
            },
        );
        let mut rect = item.rcItem;
        let mut chars: Vec<_> = text(item.hwndItem).encode_utf16().collect();
        DrawTextW(
            item.hDC,
            &mut chars,
            &mut rect,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        if item.itemState.0 & ODS_FOCUS.0 != 0 {
            rect.left += 4;
            rect.right -= 4;
            rect.top += 4;
            rect.bottom -= 4;
            let _ = DrawFocusRect(item.hDC, &rect);
        }
        SelectObject(item.hDC, old);
    }
}
