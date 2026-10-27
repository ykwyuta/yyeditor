use super::*;
pub fn rgb(c: Color) -> COLORREF {
    COLORREF(u32::from(c[0]) | (u32::from(c[1]) << 8) | (u32::from(c[2]) << 16))
}
pub unsafe fn fill_rect(dc: HDC, r: RECT, c: Color) {
    unsafe {
        let brush = CreateSolidBrush(rgb(c));
        FillRect(dc, &r, brush);
        let _ = DeleteObject(brush.into());
    }
}
pub unsafe fn text(dc: HDC, r: RECT, s: &str, c: Color) {
    unsafe {
        SetTextColor(dc, rgb(c));
        SetBkMode(dc, TRANSPARENT);
        let mut utf: Vec<_> = s.encode_utf16().collect();
        DrawTextW(
            dc,
            &mut utf,
            &mut r.clone(),
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
        );
    }
}
pub fn show_raster(dc: HDC, r: RECT, image: &Raster) {
    unsafe {
        let mut data = Vec::with_capacity(image.pixels.len() * 4);
        for p in &image.pixels {
            data.extend([p[2], p[1], p[0], 255]);
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: image.width as i32,
                biHeight: -(image.height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        StretchDIBits(
            dc,
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            0,
            0,
            image.width as i32,
            image.height as i32,
            Some(data.as_ptr().cast()),
            &info,
            DIB_RGB_COLORS,
            SRCCOPY,
        );
    }
}
pub fn paint(hwnd: HWND, kind: u32) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let target = BeginPaint(hwnd, &mut ps);
        let mut bounds = RECT::default();
        let _ = GetClientRect(hwnd, &mut bounds);
        let dc = CreateCompatibleDC(Some(target));
        let bitmap = CreateCompatibleBitmap(target, bounds.right.max(1), bounds.bottom.max(1));
        let old = SelectObject(dc, bitmap.into());
        print(hwnd, kind, dc);
        let _ = BitBlt(
            target,
            0,
            0,
            bounds.right,
            bounds.bottom,
            Some(dc),
            0,
            0,
            SRCCOPY,
        );
        SelectObject(dc, old);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(dc);
        let _ = EndPaint(hwnd, &ps);
    }
}
pub fn print(hwnd: HWND, kind: u32, dc: HDC) {
    unsafe {
        let saved = SaveDC(dc);
        let mut bounds = RECT::default();
        let _ = GetClientRect(hwnd, &mut bounds);
        fill_rect(dc, bounds, [245, 247, 250, 255]);
        with(|a| {
            let old_font = SelectObject(dc, a.font.into());
            if kind == 0 {
                paint_palette(a, dc);
            } else if kind == 3 {
                let _ = OffsetViewportOrgEx(dc, 0, -a.s(40) - a.scroll, None);
                paint_panel(a, dc);
            } else if kind == 1 {
                paint_canvas(a, dc, bounds);
            } else {
                if let Some(image) = &a.preview_image {
                    show_raster(dc, a.preview_content(), image);
                } else {
                    text(
                        dc,
                        RECT {
                            left: a.s(18),
                            top: a.s(32),
                            ..bounds
                        },
                        "ドット絵を描くと立体が表示されます",
                        [80, 95, 115, 255],
                    );
                }
                fill_rect(
                    dc,
                    RECT {
                        left: 0,
                        top: 0,
                        right: bounds.right,
                        bottom: a.preview_content().top,
                    },
                    [230, 235, 244, 255],
                );
                text(
                    dc,
                    RECT {
                        left: a.s(8),
                        top: 0,
                        right: bounds.right,
                        bottom: a.s(29),
                    },
                    &format!(
                        "3D · {} · ドラッグで回転 / ホイールでズーム",
                        a.viewpoint.map(|p| p.label()).unwrap_or("自由回転")
                    ),
                    [46, 62, 87, 255],
                );
            }
            SelectObject(dc, old_font);
        });
        let _ = RestoreDC(dc, saved);
    }
}
unsafe fn paint_panel(a: &App, dc: HDC) {
    unsafe {
        for (label, y) in [
            ("オブジェクト", 44),
            ("名前", 100),
            ("幅 / 高さ / 厚み", 152),
            ("方向（最大6枚）", 232),
            ("レイヤー", 312),
            ("不透明度 0–255", 418),
            ("ツール", 474),
            ("筆幅 1–16", 526),
            ("色 RGBA (16進数)", 578),
            ("写真・イラスト変換", 660),
        ] {
            text(
                dc,
                RECT {
                    left: a.s(12),
                    top: a.s(y),
                    right: a.s(202),
                    bottom: a.s(y + 20),
                },
                label,
                [80, 95, 115, 255],
            );
        }
        let r = RECT {
            left: a.s(12),
            top: a.s(760),
            right: a.s(202),
            bottom: a.s(782),
        };
        text(
            dc,
            r,
            &format!(
                "丸み {}%  ·  α ≥ {}",
                (a.editor.project.object().roundness * 100.).round(),
                a.editor.project.object().alpha_cutoff
            ),
            [80, 95, 115, 255],
        );
    }
}
unsafe fn paint_palette(a: &App, dc: HDC) {
    unsafe {
        let start = a.s(234);
        let top = a.s(4);
        let cell = a.s(20);
        for (i, color) in a
            .editor
            .project
            .palette
            .iter()
            .skip(a.palette_page * 32)
            .take(32)
            .enumerate()
        {
            let x = start + (i % 16) as i32 * cell;
            let y = top + (i / 16) as i32 * cell;
            fill_rect(
                dc,
                RECT {
                    left: x,
                    top: y,
                    right: x + cell - 2,
                    bottom: y + cell - 2,
                },
                *color,
            );
            if *color == a.color {
                let brush = GetSysColorBrush(COLOR_HIGHLIGHT);
                FrameRect(
                    dc,
                    &RECT {
                        left: x - 1,
                        top: y - 1,
                        right: x + cell - 1,
                        bottom: y + cell - 1,
                    },
                    brush,
                );
            }
        }
    }
}
unsafe fn paint_canvas(a: &App, dc: HDC, bounds: RECT) {
    unsafe {
        fill_rect(dc, bounds, [221, 227, 236, 255]);
        let Some(v) = a.view() else {
            return;
        };
        let max_depth = a.direction.ray_length(a.editor.project.object().size);
        let pixels = if let Some(depths) = &a.depths {
            depths
                .iter()
                .map(|&n| {
                    if n == 0 {
                        CLEAR
                    } else {
                        [
                            (25 + 160 * n / max_depth) as u8,
                            (65 + 145 * n / max_depth) as u8,
                            (160 + 85 * n / max_depth) as u8,
                            255,
                        ]
                    }
                })
                .collect()
        } else {
            v.composite()
        };
        let origin = a.origin();
        let zoom = a.zoom;
        let left = (((-origin[0]) / zoom).floor() as i32).clamp(0, v.width as i32);
        let right =
            (((bounds.right as f32 - origin[0]) / zoom).ceil() as i32).clamp(0, v.width as i32);
        let top = (((-origin[1]) / zoom).floor() as i32).clamp(0, v.height as i32);
        let bottom =
            (((bounds.bottom as f32 - origin[1]) / zoom).ceil() as i32).clamp(0, v.height as i32);
        let checker_pixels = pixels
            .iter()
            .enumerate()
            .map(|(i, &pixel)| {
                let x = i % v.width as usize;
                let y = i / v.width as usize;
                over(
                    if (x + y).is_multiple_of(2) {
                        [255, 255, 255, 255]
                    } else {
                        [232, 236, 241, 255]
                    },
                    pixel,
                    255,
                )
            })
            .collect();
        let raster = Raster {
            width: v.width,
            height: v.height,
            pixels: checker_pixels,
        };
        let _ = SetStretchBltMode(dc, COLORONCOLOR);
        show_raster(
            dc,
            RECT {
                left: origin[0].round() as i32,
                top: origin[1].round() as i32,
                right: (origin[0] + v.width as f32 * zoom).round() as i32,
                bottom: (origin[1] + v.height as f32 * zoom).round() as i32,
            },
            &raster,
        );
        if a.grid && zoom >= 5. {
            let pen = CreatePen(PS_SOLID, 1, COLORREF(0x00d7c9bd));
            let old = SelectObject(dc, pen.into());
            for x in left..=right {
                let px = (origin[0] + x as f32 * zoom).round() as i32;
                let _ = MoveToEx(dc, px, (origin[1] + top as f32 * zoom).round() as i32, None);
                let _ = LineTo(dc, px, (origin[1] + bottom as f32 * zoom).round() as i32);
            }
            for y in top..=bottom {
                let py = (origin[1] + y as f32 * zoom).round() as i32;
                let _ = MoveToEx(
                    dc,
                    (origin[0] + left as f32 * zoom).round() as i32,
                    py,
                    None,
                );
                let _ = LineTo(dc, (origin[0] + right as f32 * zoom).round() as i32, py);
            }
            SelectObject(dc, old);
            let _ = DeleteObject(pen.into());
        }
        if let Some(depths) = &a.depths
            && zoom >= a.s(12 + max_depth.to_string().len() as i32 * 8) as f32
        {
            for y in top..bottom {
                for x in left..right {
                    let count = depths[(y as u32 * v.width + x as u32) as usize];
                    if count == 0 {
                        continue;
                    }
                    let l = (origin[0] + x as f32 * zoom).round() as i32;
                    let t = (origin[1] + y as f32 * zoom).round() as i32;
                    text(
                        dc,
                        RECT {
                            left: l + a.s(2),
                            top: t,
                            right: (origin[0] + (x + 1) as f32 * zoom).round() as i32,
                            bottom: (origin[1] + (y + 1) as f32 * zoom).round() as i32,
                        },
                        &count.to_string(),
                        if count * 2 >= max_depth {
                            [23, 32, 51, 255]
                        } else {
                            [255; 4]
                        },
                    );
                }
            }
        }
        if let Some(r) = a.selection {
            let pen = CreatePen(PS_DASH, 1, COLORREF(0x00c85518));
            let old = SelectObject(dc, pen.into());
            let old_brush = SelectObject(dc, GetStockObject(NULL_BRUSH));
            let _ = Rectangle(
                dc,
                (origin[0] + r.x as f32 * zoom) as i32,
                (origin[1] + r.y as f32 * zoom) as i32,
                (origin[0] + (r.x + r.w as i32) as f32 * zoom) as i32,
                (origin[1] + (r.y + r.h as i32) as f32 * zoom) as i32,
            );
            SelectObject(dc, old_brush);
            SelectObject(dc, old);
            let _ = DeleteObject(pen.into());
        }
        let title = if a.depths.is_some() {
            format!(
                "{} · 残りの厚み 0〜{} · T:削る / U:戻す",
                a.direction.label(),
                max_depth
            )
        } else {
            format!(
                "{} · {} × {}  · {:.0}%",
                a.direction.label(),
                v.width,
                v.height,
                a.zoom * 100.
            )
        };
        text(
            dc,
            RECT {
                left: a.s(8),
                top: 0,
                right: bounds.right,
                bottom: a.s(26),
            },
            &title,
            [60, 75, 95, 255],
        );
    }
}
