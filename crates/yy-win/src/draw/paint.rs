use windows::{
    Win32::{
        Foundation::HWND,
        Graphics::{
            Direct2D::{Common::*, *},
            DirectWrite::*,
            Dxgi::Common::*,
        },
    },
    core::{HSTRING, Result, w},
};
use windows_numerics::Vector2;
use yy_draw::Point;
pub struct Layer<'a> {
    pub pixels: &'a [u8],
    pub size: (u32, u32),
    pub position: Point,
    pub destination: (f64, f64),
    pub key: String,
    pub opacity: f32,
}
pub struct View<'a> {
    pub viewport: yy_draw::Rect,
    pub grid: Option<f64>,
    pub strokes: Vec<Stroke>,
    pub ports: Vec<PortMarker>,
    pub base: Layer<'a>,
    pub sprite: Option<Layer<'a>>,
    pub origin: Point,
    pub origin_label: String,
}
pub struct Stroke {
    pub points: Vec<Point>,
    pub color: u32,
    pub width: f32,
    pub arrow_size: f32,
    pub corner_radius: f64,
}
pub struct PortMarker {
    pub position: Point,
    pub active: bool,
    pub radius: f32,
}
pub struct Painter {
    factory: ID2D1Factory,
    target: Option<ID2D1HwndRenderTarget>,
    base: Option<(String, ID2D1Bitmap)>,
    sprite: Option<(String, ID2D1Bitmap)>,
    text: IDWriteTextFormat,
    target_size: (u32, u32),
}
impl Painter {
    pub fn new() -> Result<Self> {
        unsafe {
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let fonts = crate::font::collection(&dwrite).ok();
            let text = dwrite.CreateTextFormat(
                &HSTRING::from(crate::font::BUNDLED_FAMILY),
                fonts.as_ref(),
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                12.,
                w!("ja-jp"),
            )?;
            Ok(Self {
                factory: D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?,
                target: None,
                base: None,
                sprite: None,
                text,
                target_size: (0, 0),
            })
        }
    }
    pub fn paint(
        &mut self,
        hwnd: HWND,
        size: (u32, u32),
        view: &View<'_>,
        overlay: &[(Point, Point, u32)],
    ) -> Result<()> {
        let (width, height) = size;
        unsafe {
            if self.target.is_none() {
                self.target = Some(self.factory.CreateHwndRenderTarget(
                    &D2D1_RENDER_TARGET_PROPERTIES {
                        dpiX: 96.,
                        dpiY: 96.,
                        ..Default::default()
                    },
                    &D2D1_HWND_RENDER_TARGET_PROPERTIES {
                        hwnd,
                        pixelSize: D2D_SIZE_U { width, height },
                        presentOptions: D2D1_PRESENT_OPTIONS_IMMEDIATELY,
                    },
                )?);
                self.base = None;
                self.sprite = None;
                self.target_size = size;
            }
            let t = self.target.as_ref().unwrap();
            if self.target_size != size {
                t.Resize(&D2D_SIZE_U { width, height })?;
                self.target_size = size;
            }
            update_texture(t, &mut self.base, &view.base)?;
            if let Some(layer) = &view.sprite {
                update_texture(t, &mut self.sprite, layer)?;
            }
            t.BeginDraw();
            t.Clear(Some(&D2D1_COLOR_F {
                r: 0.96,
                g: 0.97,
                b: 0.98,
                a: 1.,
            }));
            let r = view.viewport;
            let clip = D2D_RECT_F {
                left: r.x as f32,
                top: r.y as f32,
                right: (r.x + r.w) as f32,
                bottom: (r.y + r.h) as f32,
            };
            t.PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            let background = t.CreateSolidColorBrush(&rgb(0xffffff), None)?;
            t.FillRectangle(&clip, &background);
            if let Some(mut step) = view.grid {
                while (view.viewport.w + view.viewport.h) / step > 1000. {
                    step *= 5.;
                }
                let brush = t.CreateSolidColorBrush(&rgb(0xdfe7f1), None)?;
                let mut x = view.viewport.x + (view.origin.x - view.viewport.x).rem_euclid(step);
                while x < view.viewport.x + view.viewport.w {
                    t.DrawLine(
                        Vector2 {
                            X: x as f32,
                            Y: clip.top,
                        },
                        Vector2 {
                            X: x as f32,
                            Y: clip.bottom,
                        },
                        &brush,
                        0.5,
                        None,
                    );
                    x += step;
                }
                let mut y = view.viewport.y + (view.origin.y - view.viewport.y).rem_euclid(step);
                while y < view.viewport.y + view.viewport.h {
                    t.DrawLine(
                        Vector2 {
                            X: clip.left,
                            Y: y as f32,
                        },
                        Vector2 {
                            X: clip.right,
                            Y: y as f32,
                        },
                        &brush,
                        0.5,
                        None,
                    );
                    y += step;
                }
            }
            if let Some((_, bitmap)) = &self.base {
                draw_layer(t, bitmap, &view.base);
            }
            if let (Some(layer), Some((_, bitmap))) = (&view.sprite, &self.sprite) {
                draw_layer(t, bitmap, layer);
            }
            let mut brushes = std::collections::HashMap::new();
            for (a, b, color) in overlay {
                if !brushes.contains_key(color) {
                    brushes.insert(*color, t.CreateSolidColorBrush(&rgb(*color), None)?);
                }
                let brush = &brushes[color];
                t.DrawLine(
                    Vector2 {
                        X: a.x as f32,
                        Y: a.y as f32,
                    },
                    Vector2 {
                        X: b.x as f32,
                        Y: b.y as f32,
                    },
                    brush,
                    1.,
                    None,
                );
            }
            for stroke in &view.strokes {
                let brush = t.CreateSolidColorBrush(&rgb(stroke.color), None)?;
                if let Some(first) = stroke.points.first() {
                    let geometry = self.factory.CreatePathGeometry()?;
                    let sink = geometry.Open()?;
                    let vector = |p: Point| Vector2 {
                        X: p.x as f32,
                        Y: p.y as f32,
                    };
                    sink.BeginFigure(vector(*first), D2D1_FIGURE_BEGIN_HOLLOW);
                    for i in 1..stroke.points.len() {
                        if let Some((entry, exit)) = yy_draw::geometry::rounded_corner(
                            &stroke.points,
                            i,
                            stroke.corner_radius,
                        ) {
                            sink.AddLine(vector(entry));
                            sink.AddQuadraticBezier(&D2D1_QUADRATIC_BEZIER_SEGMENT {
                                point1: vector(stroke.points[i]),
                                point2: vector(exit),
                            });
                        } else {
                            sink.AddLine(vector(stroke.points[i]));
                        }
                    }
                    sink.EndFigure(D2D1_FIGURE_END_OPEN);
                    sink.Close()?;
                    t.DrawGeometry(&geometry, &brush, stroke.width, None);
                }
                if let Some(p) = stroke
                    .points
                    .windows(2)
                    .rev()
                    .find(|p| p[0].distance(p[1]) > 0.01)
                {
                    let a = p[0];
                    let b = p[1];
                    let len = a.distance(b);
                    let size = stroke.arrow_size as f64;
                    let dx = (b.x - a.x) / len;
                    let dy = (b.y - a.y) / len;
                    let points = [
                        b,
                        Point::new(
                            b.x - dx * size - dy * size / 2.,
                            b.y - dy * size + dx * size / 2.,
                        ),
                        Point::new(
                            b.x - dx * size + dy * size / 2.,
                            b.y - dy * size - dx * size / 2.,
                        ),
                    ];
                    let geometry = self.factory.CreatePathGeometry()?;
                    let sink = geometry.Open()?;
                    sink.BeginFigure(
                        Vector2 {
                            X: points[0].x as f32,
                            Y: points[0].y as f32,
                        },
                        D2D1_FIGURE_BEGIN_FILLED,
                    );
                    for p in &points[1..] {
                        sink.AddLine(Vector2 {
                            X: p.x as f32,
                            Y: p.y as f32,
                        });
                    }
                    sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                    sink.Close()?;
                    t.FillGeometry(&geometry, &brush, None);
                }
            }
            if !view.ports.is_empty() {
                let blue = t.CreateSolidColorBrush(&rgb(0x287bdb), None)?;
                let white = t.CreateSolidColorBrush(&rgb(0xffffff), None)?;
                for port in &view.ports {
                    let radius = port.radius * if port.active { 1.5 } else { 1. };
                    let circle = D2D1_ELLIPSE {
                        point: Vector2 {
                            X: port.position.x as f32,
                            Y: port.position.y as f32,
                        },
                        radiusX: radius,
                        radiusY: radius,
                    };
                    t.FillEllipse(&circle, if port.active { &blue } else { &white });
                    t.DrawEllipse(&circle, &blue, 1., None);
                }
            }
            let brush = t.CreateSolidColorBrush(&rgb(0x537081), None)?;
            let origin = view.origin;
            let visible = origin.x >= clip.left as f64
                && origin.x <= clip.right as f64
                && origin.y >= clip.top as f64
                && origin.y <= clip.bottom as f64;
            if visible {
                t.DrawEllipse(
                    &D2D1_ELLIPSE {
                        point: Vector2 {
                            X: origin.x as f32,
                            Y: origin.y as f32,
                        },
                        radiusX: 4.,
                        radiusY: 4.,
                    },
                    &brush,
                    1.,
                    None,
                );
            }
            let x = if visible {
                origin.x as f32 + 8.
            } else {
                clip.left + 10.
            }
            .min((clip.right - 260.).max(clip.left));
            let y = if visible {
                origin.y as f32 + 6.
            } else {
                clip.top + 8.
            }
            .min((clip.bottom - 22.).max(clip.top));
            let label: Vec<u16> = view.origin_label.encode_utf16().collect();
            let r = D2D_RECT_F {
                left: x,
                top: y,
                right: (x
                    + view
                        .origin_label
                        .chars()
                        .map(|c| if c.is_ascii() { 6. } else { 12. })
                        .sum::<f32>()
                    + 8.)
                    .min(clip.right),
                bottom: y + 20.,
            };
            let white = t.CreateSolidColorBrush(
                &D2D1_COLOR_F {
                    r: 1.,
                    g: 1.,
                    b: 1.,
                    a: 0.88,
                },
                None,
            )?;
            t.FillRectangle(&r, &white);
            t.DrawText(
                &label,
                &self.text,
                &r,
                &brush,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
            t.PopAxisAlignedClip();
            let result = t.EndDraw(None, None);
            if result.is_err() {
                self.target = None;
                self.base = None;
                self.sprite = None;
            }
            result
        }
    }
}
fn rgb(c: u32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: ((c >> 16) & 255) as f32 / 255.,
        g: ((c >> 8) & 255) as f32 / 255.,
        b: (c & 255) as f32 / 255.,
        a: 1.,
    }
}
fn update_texture(
    t: &ID2D1HwndRenderTarget,
    cache: &mut Option<(String, ID2D1Bitmap)>,
    layer: &Layer<'_>,
) -> Result<()> {
    if layer.size.0 == 0 || layer.size.1 == 0 {
        *cache = None;
        return Ok(());
    }
    if cache.as_ref().is_some_and(|(key, _)| *key == layer.key) {
        return Ok(());
    }
    let bitmap = unsafe {
        t.CreateBitmap(
            D2D_SIZE_U {
                width: layer.size.0,
                height: layer.size.1,
            },
            Some(layer.pixels.as_ptr().cast()),
            layer.size.0 * 4,
            &D2D1_BITMAP_PROPERTIES {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.,
                dpiY: 96.,
            },
        )?
    };
    *cache = Some((layer.key.clone(), bitmap));
    Ok(())
}
unsafe fn draw_layer(t: &ID2D1HwndRenderTarget, bitmap: &ID2D1Bitmap, layer: &Layer<'_>) {
    unsafe {
        t.DrawBitmap(
            bitmap,
            Some(&D2D_RECT_F {
                left: layer.position.x as f32,
                top: layer.position.y as f32,
                right: (layer.position.x + layer.destination.0) as f32,
                bottom: (layer.position.y + layer.destination.1) as f32,
            }),
            layer.opacity,
            D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
            None,
        );
    }
}
