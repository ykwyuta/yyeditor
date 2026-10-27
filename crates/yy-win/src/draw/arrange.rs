use super::*;
pub(super) const ITEMS: &[(u16, &str)] = &[
    (GROUP, "グループ化\tCtrl+G"),
    (UNGROUP, "グループ解除\tCtrl+Shift+G"),
    (FRONT, "最前面へ"),
    (BACK, "最背面へ"),
    (ALIGN_LEFT, "左揃え"),
    (CENTER_X, "左右中央揃え"),
    (ALIGN_RIGHT, "右揃え"),
    (ALIGN_TOP, "上揃え"),
    (CENTER_Y, "上下中央揃え"),
    (ALIGN_BOTTOM, "下揃え"),
    (DISTRIBUTE, "左右に均等配置"),
    (DISTRIBUTE_Y, "上下に均等配置"),
    (ALIGN_SELECTION, "選択したオブジェクトを基準に整列"),
    (ALIGN_PAGE, "ページを基準に整列"),
];
fn action(id: u16) -> Option<layout::Align> {
    use layout::Align::*;
    Some(match id {
        ALIGN_LEFT => Left,
        CENTER_X => Center,
        ALIGN_RIGHT => Right,
        ALIGN_TOP => Top,
        CENTER_Y => Middle,
        ALIGN_BOTTOM => Bottom,
        DISTRIBUTE => Horizontal,
        DISTRIBUTE_Y => Vertical,
        _ => return None,
    })
}
pub(super) fn handle(_hwnd: HWND, id: u16) -> bool {
    if !matches!(id, GROUP | UNGROUP | ALIGN_SELECTION | ALIGN_PAGE) && action(id).is_none() {
        return false;
    }
    with(|a| {
        if matches!(id, ALIGN_SELECTION | ALIGN_PAGE) {
            a.layout_reference = if id == ALIGN_PAGE {
                layout::Reference::Page
            } else {
                layout::Reference::Selection
            };
            a.status(if id == ALIGN_PAGE {
                "ページを基準に整列します"
            } else {
                "選択範囲を基準に整列します"
            });
            return;
        }
        let reference = a.layout_reference;
        let tab = a.current();
        let before = tab.doc.clone();
        let result = match id {
            GROUP => tab.doc.group_objects(&tab.selected).map(|ids| {
                tab.selected = ids;
            }),
            UNGROUP => {
                tab.selected = tab.doc.ungroup_objects(&tab.selected);
                Ok(())
            }
            _ => tab
                .doc
                .arrange(&tab.selected, action(id).unwrap(), reference)
                .map(|_| ()),
        };
        if let Err(e) = result {
            a.status(&e);
            return;
        }
        tab.history.commit(before, &tab.doc);
        a.properties();
        a.refresh_tabs();
        a.invalidate();
    });
    true
}
pub(super) fn context_menu(hwnd: HWND, screen: Point) {
    let target = with(|a| unsafe {
        if a.drag.is_some() {
            return None;
        }
        let (l, t, w, h) = a.viewport();
        let mut p = windows::Win32::Foundation::POINT {
            x: screen.x as i32,
            y: screen.y as i32,
        };
        if p.x == -1 && p.y == -1 {
            p.x = l + 80;
            p.y = t + 80;
        } else {
            let _ = windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut p);
        }
        if p.x < l || p.y < t || p.x >= l + w as i32 || p.y >= t + h as i32 {
            return None;
        }
        let q = a.document_point(Point::new(p.x as f64, p.y as f64));
        let tab = a.current();
        if let Some(hit) = geometry::hit(&tab.doc, q, 6. / tab.zoom)
            && !tab.selected.contains(&hit)
        {
            tab.selected = tab.doc.expanded_selection(&[hit]);
            a.properties();
            a.invalidate();
        }
        let menu = CreatePopupMenu().ok()?;
        for &(id, label) in ITEMS {
            if AppendMenuW(menu, MF_STRING, id as usize, &HSTRING::from(label)).is_err() {
                let _ = DestroyMenu(menu);
                return None;
            }
        }
        a.update_menu(menu);
        let _ = windows::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut p);
        Some((menu, p))
    })
    .flatten();
    if let Some((menu, p)) = target {
        unsafe {
            let id = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON,
                p.x,
                p.y,
                Some(0),
                hwnd,
                None,
            )
            .0 as u16;
            let _ = DestroyMenu(menu);
            if id != 0 {
                command(hwnd, id);
            }
        }
    }
}
