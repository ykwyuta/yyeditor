//! Native regression probe waits for completed worker generations, never sleeps.
use super::*;
use windows::core::BOOL;
fn check(condition: bool, message: &str) -> io::Result<()> {
    if condition {
        Ok(())
    } else {
        Err(io::Error::other(message))
    }
}
fn xy(x: i32, y: i32) -> LPARAM {
    LPARAM(((x as u32 & 0xffff) | ((y as u32 & 0xffff) << 16)) as isize)
}
pub fn advance() -> io::Result<()> {
    let (phase, started) = with(|a| (a.smoke_phase, a.smoke_started)).unwrap();
    if started.elapsed() > std::time::Duration::from_secs(50) {
        return Err(io::Error::other("YY3Designer UI smoke timed out"));
    }
    match phase {
        0 => {
            with(|a| -> io::Result<()> {
                a.smoke_phase = 1;
                let dir = a.smoke.as_ref().unwrap();
                std::fs::create_dir_all(dir)?;
                let mut p = Project::default();
                p.objects[0] = Object::new("検証オブジェクト", [16, 16, 4])?;
                let v = p.object_mut().enable(Direction::Front);
                shape(
                    v,
                    Tool::Rectangle,
                    [[3, 3], [12, 12]],
                    [70, 130, 220, 255],
                    1,
                    true,
                    None,
                );
                shape(
                    v,
                    Tool::Ellipse,
                    [[5, 5], [10, 10]],
                    [250, 170, 55, 255],
                    1,
                    true,
                    None,
                );
                p.object_mut().roundness = 0.3;
                for d in Direction::ALL.into_iter().skip(1) {
                    let v = mesh::projected_view(p.object(), d)?;
                    p.object_mut().views[d.index()] = Some(v);
                }
                a.editor = Editor::new(p);
                a.choose_object(0);
                a.color = [250, 40, 30, 255];
                a.color_text();
                a.tool = Tool::Pencil;
                check(
                    a.text(VIEW).contains("正面"),
                    "Direction combobox has no selected label",
                )?;
                check(
                    a.text(OBJECT).contains("検証オブジェクト"),
                    "Object combobox has no selected label",
                )?;
                check(
                    a.text(TOOL).contains("鉛筆"),
                    "Tool combobox has no selected label",
                )?;
                Ok(())
            })
            .unwrap()?;
            let (canvas, start, end) = with(|a| {
                let o = a.origin();
                let pixel =
                    |x: f32, y: f32| [(o[0] + x * a.zoom) as i32, (o[1] + y * a.zoom) as i32];
                (a.canvas, pixel(6.5, 6.5), pixel(8.5, 6.5))
            })
            .unwrap();
            unsafe {
                SendMessageW(
                    canvas,
                    WM_LBUTTONDOWN,
                    Some(WPARAM(1)),
                    Some(xy(start[0], start[1])),
                );
                SendMessageW(
                    canvas,
                    WM_MOUSEMOVE,
                    Some(WPARAM(1)),
                    Some(xy(end[0], end[1])),
                );
                SendMessageW(
                    canvas,
                    WM_LBUTTONUP,
                    Some(WPARAM(0)),
                    Some(xy(end[0], end[1])),
                );
            }
            check(
                with(|a| a.view().unwrap().layers[0].pixels[6 * 16 + 8] == [250, 40, 30, 255])
                    .unwrap(),
                "Native pencil stroke was not applied",
            )?;
            dispatch(UNDO, 0)?;
            check(
                with(|a| a.view().unwrap().layers[0].pixels[6 * 16 + 8] != [250, 40, 30, 255])
                    .unwrap(),
                "Undo did not undo a whole stroke",
            )?;
            dispatch(REDO, 0)?;
            select_tool(Tool::Thin);
            canvas_stroke([6, 6], [8, 6]);
            check(
                with(|a| {
                    a.editor.project.object().carved.len() == 3
                        && a.depths.as_ref().unwrap()[6 * 16 + 6] == 3
                })
                .unwrap(),
                "Thickness brush did not remove exactly one voxel per pixel",
            )?;
            dispatch(UNDO, 0)?;
            check(
                with(|a| a.editor.project.object().carved.is_empty()).unwrap(),
                "Thickness undo lost the original shape",
            )?;
            dispatch(REDO, 0)?;
            canvas_stroke([6, 6], [6, 6]);
            check(
                with(|a| a.depths.as_ref().unwrap()[6 * 16 + 6] == 2).unwrap(),
                "A second stroke did not thin the object again",
            )?;
            select_tool(Tool::RestoreDepth);
            canvas_stroke([6, 6], [6, 6]);
            check(
                with(|a| {
                    a.depths.as_ref().unwrap()[6 * 16 + 6] == 4
                        && a.editor.project.object().carved.len() == 2
                })
                .unwrap(),
                "Restore-depth brush did not restore the selected column",
            )?;
            dispatch(UNDO, 0)?;
            select_tool(Tool::Thin);
            dispatch(RESET_DEPTH, 0)?;
            check(
                with(|a| a.editor.project.object().carved.is_empty()).unwrap(),
                "Reset thickness did not restore the object",
            )?;
            dispatch(UNDO, 0)?;
            check(
                with(|a| a.view().unwrap().layers[0].pixels[6 * 16 + 6] == [250, 40, 30, 255])
                    .unwrap(),
                "Thickness editing modified the source image",
            )?;
            with(|a| -> io::Result<()> {
                let dir = a.smoke.as_ref().unwrap();
                yy3_core::save(&dir.join("sample.yy3d"), &a.editor.project)?;
                let restored = yy3_core::load(&dir.join("sample.yy3d"))?;
                check(
                    restored == a.editor.project,
                    "Project roundtrip lost information",
                )?;
                image::write_png(
                    &dir.join("sprite.png"),
                    &image::export_view(a.view().unwrap(), 8, 0.)?,
                )?;
                image::write_png(
                    &dir.join("rounded.png"),
                    &image::export_view(a.view().unwrap(), 8, 0.3)?,
                )?;
                let source = read_picture(&dir.join("sprite.png"))?;
                check(
                    source.width == 128 && source.height == 128,
                    "WIC image import dimensions changed",
                )?;
                let (converted, palette) = image::convert(
                    &source,
                    16,
                    16,
                    &Convert {
                        sampling: Sampling::Nearest,
                        colors: 8,
                        dither: false,
                    },
                )?;
                check(
                    converted.len() == 256 && palette.len() <= 8,
                    "Image import and quantization failed",
                )?;
                let m = mesh::build(a.editor.project.object())?;
                mesh::export_obj(&dir.join("sample.obj"), &m)?;
                a.saved = a.editor.project.clone();
                Ok(())
            })
            .unwrap()?;
        }
        1 => {
            if !with(|a| a.rendered_generation == a.generation).unwrap() {
                return Ok(());
            }
            with(|a| -> io::Result<()> {
                check(
                    a.mesh.as_ref().is_some_and(|m| m.voxels > 0),
                    "3D preview has no geometry",
                )?;
                image::write_png(
                    &a.smoke.as_ref().unwrap().join("preview.png"),
                    a.preview_image.as_ref().unwrap(),
                )?;
                a.smoke_phase = 5;
                Ok(())
            })
            .unwrap()?;
            click_camera(0);
        }
        5..=11 => {
            if !with(|a| a.rendered_generation == a.generation).unwrap() {
                return Ok(());
            }
            let index = (phase - 5) as usize;
            let preset = camera::Preset::ALL[index];
            with(|a| -> io::Result<()> {
                check(
                    a.viewpoint == Some(preset) && a.model_zoom == 1.,
                    "Camera preset did not reset orientation and zoom",
                )?;
                check(
                    (a.yaw, a.pitch) == preset.angles(),
                    "Camera angle differs from selected preset",
                )?;
                check(
                    a.editor.project == a.saved,
                    "Camera preset modified the document",
                )?;
                image::write_png(
                    &a.smoke
                        .as_ref()
                        .unwrap()
                        .join(format!("camera-{index}.png")),
                    a.preview_image.as_ref().unwrap(),
                )?;
                a.smoke_phase += 1;
                Ok(())
            })
            .unwrap()?;
            if index < 6 {
                click_camera(index + 1);
            } else {
                let (hwnd, x, y) = with(|a| {
                    let r = a.preview_content();
                    (a.preview, r.right / 2, (r.top + r.bottom) / 2)
                })
                .unwrap();
                unsafe {
                    SendMessageW(hwnd, WM_LBUTTONDOWN, Some(WPARAM(1)), Some(xy(x, y)));
                    SendMessageW(hwnd, WM_MOUSEMOVE, Some(WPARAM(1)), Some(xy(x + 8, y + 8)));
                    SendMessageW(hwnd, WM_LBUTTONUP, Some(WPARAM(0)), Some(xy(x + 8, y + 8)));
                }
                check(
                    with(|a| a.viewpoint.is_none()).unwrap(),
                    "Free mouse orbit is unavailable after a preset",
                )?;
                click_camera(6);
            }
        }
        12 => {
            if !with(|a| a.rendered_generation == a.generation).unwrap() {
                return Ok(());
            }
            with(|a| {
                a.smoke_phase = 2;
                unsafe {
                    let _ = SetWindowPos(
                        a.hwnd,
                        None,
                        0,
                        0,
                        a.s(780),
                        a.s(600),
                        SWP_NOMOVE | SWP_NOZORDER,
                    );
                }
                a.layout();
            });
        }
        2 => {
            if !with(|a| a.rendered_generation == a.generation).unwrap() {
                return Ok(());
            }
            let (hwnd, path) =
                with(|a| (a.hwnd, a.smoke.as_ref().unwrap().join("small.png"))).unwrap();
            capture(hwnd, &path)?;
            with(|a| -> io::Result<()> {
                check_layout(a)?;
                a.scroll_panel(a.s(500));
                check(a.scroll > 0, "Sidebar did not scroll on small window")?;
                a.smoke_phase = 3;
                unsafe {
                    let _ = SetWindowPos(
                        a.hwnd,
                        None,
                        0,
                        0,
                        a.s(1200),
                        a.s(820),
                        SWP_NOMOVE | SWP_NOZORDER,
                    );
                }
                a.layout();
                Ok(())
            })
            .unwrap()?;
        }
        3 => {
            if !with(|a| a.rendered_generation == a.generation).unwrap() {
                return Ok(());
            }
            with(|a| -> io::Result<()> {
                check_layout(a)?;
                a.scroll = 0;
                a.layout();
                a.smoke_phase = 4;

                Ok(())
            })
            .unwrap()?;
            let (hwnd, path) =
                with(|a| (a.hwnd, a.smoke.as_ref().unwrap().join("large.png"))).unwrap();
            capture(hwnd, &path)?;
            dispatch(HELP, 0)?;
        }
        4 => {
            let help = own_help();
            if help.is_invalid() {
                return Ok(());
            }
            if !unsafe { IsWindowVisible(help) }.as_bool() {
                return Ok(());
            }
            unsafe {
                let _ = DestroyWindow(help);
            }
            with(|a|->io::Result<()>{std::fs::write(a.smoke.as_ref().unwrap().join("ui-smoke.txt"),"OK: native stroke, thickness brush, depth restore, undo/redo, six views, lossless project, PNG, OBJ/MTL, seven camera presets, free orbit, small/large resize, shared Markdown help\n")?;a.smoke=None;unsafe{let _=PostMessageW(Some(a.hwnd),WM_CLOSE,WPARAM(0),LPARAM(0));}Ok(())}).unwrap()?;
        }
        _ => {}
    }
    Ok(())
}
fn check_layout(a: &App) -> io::Result<()> {
    let root = a.rect(a.hwnd);
    for child in [a.canvas, a.preview, a.panel] {
        let mut r = RECT::default();
        unsafe {
            GetWindowRect(child, &mut r).map_err(io_error)?;
            let points = std::slice::from_raw_parts_mut((&raw mut r).cast::<POINT>(), 2);
            MapWindowPoints(None, Some(a.hwnd), points);
        }
        check(
            r.left >= 0
                && r.top >= 0
                && r.right <= root.right
                && r.bottom <= root.bottom
                && r.right > r.left
                && r.bottom > r.top,
            "Child pane escaped the client rectangle",
        )?;
    }
    let bounds = a.rect(a.preview);
    let content = a.preview_content();
    for i in 0..camera::Preset::ALL.len() {
        let mut r = RECT::default();
        unsafe {
            GetWindowRect(a.controls[&(CAMERA_FIRST + i as u16)], &mut r).map_err(io_error)?;
            let p = std::slice::from_raw_parts_mut((&raw mut r).cast::<POINT>(), 2);
            MapWindowPoints(None, Some(a.preview), p);
        }
        check(
            r.left >= 0 && r.right <= bounds.right && r.top >= 0 && r.bottom <= content.top,
            "Camera button overlapped the model viewport or escaped its pane",
        )?;
    }
    Ok(())
}
fn click_camera(index: usize) {
    let button = with(|a| {
        a.model_zoom = 4.;
        a.yaw = 1.234;
        a.pitch = 0.321;
        a.controls[&(CAMERA_FIRST + index as u16)]
    })
    .unwrap();
    unsafe {
        SendMessageW(button, BM_CLICK, None, None);
    }
}
fn select_tool(tool: Tool) {
    let (combo, root) = with(|a| (a.controls[&TOOL], a.hwnd)).unwrap();
    let index = Tool::ALL.iter().position(|&t| t == tool).unwrap();
    unsafe {
        SendMessageW(combo, CB_SETCURSEL, Some(WPARAM(index)), None);
        SendMessageW(
            root,
            WM_COMMAND,
            Some(WPARAM(TOOL as usize | ((CBN_SELCHANGE as usize) << 16))),
            Some(LPARAM(combo.0 as isize)),
        );
    }
}
fn canvas_stroke(start: [i32; 2], end: [i32; 2]) {
    let (hwnd, a, b) = with(|a| {
        let origin = a.origin();
        let p = |[x, y]: [i32; 2]| {
            xy(
                (origin[0] + (x as f32 + 0.5) * a.zoom) as i32,
                (origin[1] + (y as f32 + 0.5) * a.zoom) as i32,
            )
        };
        (a.canvas, p(start), p(end))
    })
    .unwrap();
    unsafe {
        SendMessageW(hwnd, WM_LBUTTONDOWN, Some(WPARAM(1)), Some(a));
        SendMessageW(hwnd, WM_MOUSEMOVE, Some(WPARAM(1)), Some(b));
        SendMessageW(hwnd, WM_MOUSEMOVE, Some(WPARAM(1)), Some(b));
        SendMessageW(hwnd, WM_LBUTTONUP, Some(WPARAM(0)), Some(b));
    }
}
fn own_help() -> HWND {
    unsafe extern "system" fn find(hwnd: HWND, context: LPARAM) -> BOOL {
        unsafe {
            let mut title = [0u16; 128];
            let n = GetWindowTextW(hwnd, &mut title);
            if String::from_utf16_lossy(&title[..n as usize]) == "YY3Designer ヘルプ" {
                *(context.0 as *mut HWND) = hwnd;
                return BOOL(0);
            }
            BOOL(1)
        }
    }
    let mut found = HWND::default();
    if let Some(hwnd) = with(|a| a.hwnd) {
        unsafe {
            let thread = GetWindowThreadProcessId(hwnd, None);
            let _ = EnumThreadWindows(thread, Some(find), LPARAM((&raw mut found) as isize));
        }
    }
    found
}
fn capture(hwnd: HWND, path: &Path) -> io::Result<()> {
    unsafe {
        let mut r = RECT::default();
        GetClientRect(hwnd, &mut r).map_err(io_error)?;
        let source = GetDC(Some(hwnd));
        let dc = CreateCompatibleDC(Some(source));
        let bitmap = CreateCompatibleBitmap(source, r.right, r.bottom);
        let old = SelectObject(dc, bitmap.into());
        drawing::print(hwnd, 0, dc);
        capture_children(hwnd, hwnd, dc, r);
        SelectObject(dc, old);
        let mut bytes = vec![0; (r.right * r.bottom * 4) as usize];
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: r.right,
                biHeight: -r.bottom,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let copied = GetDIBits(
            dc,
            bitmap,
            0,
            r.bottom as u32,
            Some(bytes.as_mut_ptr().cast()),
            &mut info,
            DIB_RGB_COLORS,
        );
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(dc);
        ReleaseDC(Some(hwnd), source);
        check(copied != 0, "Unable to capture window")?;
        let image = Raster {
            width: r.right as u32,
            height: r.bottom as u32,
            pixels: bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|p| [p[2], p[1], p[0], 255])
                .collect(),
        };
        check(
            image.pixels.iter().any(|p| p[0] > 100 && p[1] > 100),
            "Window rendering produced a blank image",
        )?;
        image::write_png(path, &image)
    }
}
unsafe fn capture_children(parent: HWND, root: HWND, dc: HDC, clip: RECT) {
    unsafe {
        let mut child = GetWindow(parent, GW_CHILD).ok();
        while let Some(hwnd) = child {
            if GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_VISIBLE.0 != 0 {
                let mut bounds = RECT::default();
                let _ = GetWindowRect(hwnd, &mut bounds);
                let points = std::slice::from_raw_parts_mut((&raw mut bounds).cast::<POINT>(), 2);
                MapWindowPoints(None, Some(root), points);
                let intersection = RECT {
                    left: bounds.left.max(clip.left),
                    top: bounds.top.max(clip.top),
                    right: bounds.right.min(clip.right),
                    bottom: bounds.bottom.min(clip.bottom),
                };
                if intersection.right > intersection.left && intersection.bottom > intersection.top
                {
                    let saved = SaveDC(dc);
                    IntersectClipRect(
                        dc,
                        intersection.left,
                        intersection.top,
                        intersection.right,
                        intersection.bottom,
                    );
                    let mut class = [0u16; 64];
                    GetClassNameW(hwnd, &mut class);
                    let custom = String::from_utf16_lossy(&class).starts_with("YY3DesignerPane");
                    let mut origin = [POINT::default()];
                    MapWindowPoints(Some(hwnd), Some(root), &mut origin);
                    if custom {
                        let _ = SetViewportOrgEx(dc, origin[0].x, origin[0].y, None);
                        drawing::print(hwnd, GetWindowLongPtrW(hwnd, GWLP_USERDATA) as u32, dc);
                    } else {
                        let _ = SetViewportOrgEx(dc, bounds.left, bounds.top, None);
                        SendMessageW(
                            hwnd,
                            WM_PRINT,
                            Some(WPARAM(dc.0 as usize)),
                            Some(LPARAM(
                                (PRF_CLIENT | PRF_NONCLIENT | PRF_ERASEBKGND | PRF_CHILDREN)
                                    as isize,
                            )),
                        );
                    }
                    let _ = RestoreDC(dc, saved);
                    let mut client = RECT::default();
                    let _ = GetClientRect(hwnd, &mut client);
                    let client = RECT {
                        left: origin[0].x.max(intersection.left),
                        top: origin[0].y.max(intersection.top),
                        right: (origin[0].x + client.right).min(intersection.right),
                        bottom: (origin[0].y + client.bottom).min(intersection.bottom),
                    };
                    if custom {
                        capture_children(hwnd, root, dc, client);
                    }
                }
            }
            child = GetWindow(hwnd, GW_HWNDNEXT).ok();
        }
    }
}
