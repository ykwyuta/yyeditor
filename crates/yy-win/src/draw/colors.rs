//! Native color palette with RGB entry and sixteen custom swatches.
use super::*;
use windows::Win32::{Foundation::COLORREF, Graphics::Gdi::*, UI::Controls::Dialogs::*};
pub const FILL: u16 = 40;
pub const LINE: u16 = 41;
pub fn colorref(rgb: u32) -> COLORREF {
    COLORREF(((rgb & 255) << 16) | (rgb & 0xff00) | ((rgb >> 16) & 255))
}
pub fn rgb(color: COLORREF) -> u32 {
    let c = color.0;
    ((c & 255) << 16) | (c & 0xff00) | ((c >> 16) & 255)
}
pub fn choose(hwnd: HWND, target: u16) {
    let Some((initial, mut custom)) = with(|a| {
        (
            if target == 67 {
                a.export_color
            } else {
                a.active_color(target == FILL)
            },
            a.custom_colors,
        )
    }) else {
        return;
    };
    let mut dialog = CHOOSECOLORW {
        lStructSize: std::mem::size_of::<CHOOSECOLORW>() as u32,
        hwndOwner: hwnd,
        rgbResult: colorref(initial),
        lpCustColors: custom.as_mut_ptr(),
        Flags: CC_RGBINIT | CC_FULLOPEN | CC_ANYCOLOR,
        ..Default::default()
    };
    if unsafe { ChooseColorW(&mut dialog) }.as_bool() {
        let color = rgb(dialog.rgbResult);
        with(|a| {
            a.custom_colors = custom;
            if target == 67 {
                a.export_color = color;
                a.transparent = false;
                a.status("背景色を変更しました");
            } else {
                a.apply_chosen_color(target == FILL, color);
            }
        });
    }
}
impl App {
    fn active_color(&self, fill: bool) -> u32 {
        u32::from_str_radix(
            text(self.fields[if fill { 5 } else { 6 }]).trim_start_matches('#'),
            16,
        )
        .ok()
        .filter(|v| *v <= 0xffffff)
        .unwrap_or(if fill {
            self.fill_color
        } else {
            self.line_color
        })
    }
    pub(super) fn apply_chosen_color(&mut self, fill: bool, color: u32) -> bool {
        if color > 0xffffff {
            return false;
        }
        let index = if fill { 5 } else { 6 };
        unsafe {
            let _ = SetWindowTextW(self.fields[index], &HSTRING::from(format!("{color:06X}")));
        }
        self.property_dirty = true;
        if !self.apply_properties() {
            return false;
        }
        if fill {
            self.fill_color = color;
        } else {
            self.line_color = color;
        }
        self.properties();
        self.invalidate();
        true
    }
    pub(super) fn draw_color_button(&self, item: &DRAWITEMSTRUCT) {
        unsafe {
            FillRect(item.hDC, &item.rcItem, GetSysColorBrush(COLOR_BTNFACE));
            let mut r = item.rcItem;
            r.left += 4;
            r.top += 4;
            r.right = r.left + 18;
            r.bottom -= 4;
            let brush =
                CreateSolidBrush(colorref(self.active_color(item.CtlID == u32::from(FILL))));
            FillRect(item.hDC, &r, brush);
            FrameRect(item.hDC, &r, GetSysColorBrush(COLOR_BTNSHADOW));
            let _ = DeleteObject(brush.into());
            r.left = r.right + 3;
            r.right = item.rcItem.right;
            let old = SelectObject(item.hDC, self.font.into());
            SetTextColor(item.hDC, COLORREF(0x243044));
            SetBkMode(item.hDC, TRANSPARENT);
            let mut label: Vec<u16> = "色".encode_utf16().collect();
            DrawTextW(
                item.hDC,
                &mut label,
                &mut r,
                DT_SINGLELINE | DT_CENTER | DT_VCENTER,
            );
            SelectObject(item.hDC, old);
            if item.itemState.0 & ODS_FOCUS.0 != 0 {
                let _ = DrawFocusRect(item.hDC, &item.rcItem);
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn palette_roundtrips_rgb_and_windows_bgr() {
        assert_eq!(colorref(0x123456).0, 0x563412);
        assert_eq!(rgb(colorref(0x123456)), 0x123456);
    }
}
