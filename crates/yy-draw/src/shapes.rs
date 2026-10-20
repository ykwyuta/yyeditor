//! Editable, font-independent shapes. Canonical contours are cached and shared
//! by rendering, picking and ports, so their geometry stays consistent.
use crate::*;
use std::{collections::HashMap, fmt::Write, sync::OnceLock};

pub const CATEGORIES: &[&str] = &[
    "基本図形 / 図形",
    "基本図形 / 演算子",
    "基本図形 / かっこ",
    "基本図形 / ブロック矢印",
    "基本図形 / 吹き出し",
    "基本図形 / フローチャート",
];
impl Shape {
    pub fn is_extended(self) -> bool {
        !matches!(
            self,
            Self::Rectangle
                | Self::Rounded
                | Self::Ellipse
                | Self::Diamond
                | Self::Triangle
                | Self::Cylinder
                | Self::Cloud
                | Self::Text
                | Self::Frame
                | Self::Icon
                | Self::Junction
        )
    }
}
pub fn category(shape: Shape) -> &'static str {
    match shape {
        Shape::Plus
        | Shape::Minus
        | Shape::Multiply
        | Shape::Divide
        | Shape::Equals
        | Shape::NotEqual => CATEGORIES[1],
        Shape::LeftBrace
        | Shape::RightBrace
        | Shape::BracePair
        | Shape::LeftBracket
        | Shape::RightBracket
        | Shape::BracketPair => CATEGORIES[2],
        Shape::ArrowRight
        | Shape::ArrowLeft
        | Shape::ArrowUp
        | Shape::ArrowDown
        | Shape::ArrowBoth
        | Shape::ArrowUpDown
        | Shape::ArrowFourWay
        | Shape::ArrowBent
        | Shape::ArrowUTurn
        | Shape::Chevron => CATEGORIES[3],
        Shape::CalloutRect
        | Shape::CalloutRounded
        | Shape::CalloutOval
        | Shape::CalloutCloud
        | Shape::CalloutThought => CATEGORIES[4],
        Shape::FlowProcessDouble
        | Shape::FlowDocument
        | Shape::FlowMultiDocument
        | Shape::FlowTerminator
        | Shape::FlowManualInput
        | Shape::FlowPreparation
        | Shape::FlowOffpage
        | Shape::FlowDelay => CATEGORIES[5],
        _ => CATEGORIES[0],
    }
}
pub fn default_size(shape: Shape) -> (f64, f64) {
    use Shape::*;
    match shape {
        Junction => (8., 8.),
        Icon => (80., 96.),
        Plus | Minus | Multiply | Divide | Equals | NotEqual | Pentagon | Hexagon | Octagon
        | Star4 | Star5 | Star6 | Star8 | Heart | Moon | Sun | Donut => (80., 80.),
        LeftBrace | RightBrace | LeftBracket | RightBracket => (32., 100.),
        CalloutRect | CalloutRounded | CalloutOval | CalloutCloud | CalloutThought => (144., 112.),
        _ => (144., 80.),
    }
}
pub fn search_terms(shape: Shape) -> &'static str {
    match shape {
        Shape::Plus => "+ 足し算",
        Shape::Minus => "- 引き算",
        Shape::Multiply => "* 掛け算",
        Shape::Divide => "/ 割り算",
        Shape::Equals => "= イコール",
        Shape::NotEqual => "!= <> ノットイコール",
        _ => "",
    }
}
#[derive(Clone)]
enum Command {
    Move(Point),
    Line(Point),
    Quad(Point, Point),
    Cubic(Point, Point, Point),
    Close,
}
#[derive(Default)]
pub struct Path {
    commands: Vec<Command>,
    contours: Vec<Vec<Point>>,
}
impl Path {
    fn m(&mut self, x: f64, y: f64) -> &mut Self {
        let p = Point::new(x, y);
        self.commands.push(Command::Move(p));
        self.contours.push(vec![p]);
        self
    }
    fn l(&mut self, x: f64, y: f64) -> &mut Self {
        let p = Point::new(x, y);
        self.commands.push(Command::Line(p));
        self.contours.last_mut().unwrap().push(p);
        self
    }
    fn q(&mut self, x: f64, y: f64, ex: f64, ey: f64) -> &mut Self {
        let a = *self.contours.last().unwrap().last().unwrap();
        let b = Point::new(x, y);
        let c = Point::new(ex, ey);
        self.commands.push(Command::Quad(b, c));
        for i in 1..=24 {
            let t = i as f64 / 24.;
            let s = 1. - t;
            self.contours.last_mut().unwrap().push(Point::new(
                s * s * a.x + 2. * s * t * b.x + t * t * c.x,
                s * s * a.y + 2. * s * t * b.y + t * t * c.y,
            ));
        }
        self
    }
    fn c(&mut self, x: f64, y: f64, x2: f64, y2: f64, ex: f64, ey: f64) -> &mut Self {
        let a = *self.contours.last().unwrap().last().unwrap();
        let b = Point::new(x, y);
        let c = Point::new(x2, y2);
        let d = Point::new(ex, ey);
        self.commands.push(Command::Cubic(b, c, d));
        for i in 1..=24 {
            let t = i as f64 / 24.;
            let s = 1. - t;
            self.contours.last_mut().unwrap().push(Point::new(
                s * s * s * a.x + 3. * s * s * t * b.x + 3. * s * t * t * c.x + t * t * t * d.x,
                s * s * s * a.y + 3. * s * s * t * b.y + 3. * s * t * t * c.y + t * t * t * d.y,
            ));
        }
        self
    }
    fn z(&mut self) -> &mut Self {
        self.commands.push(Command::Close);
        let contour = self.contours.last_mut().unwrap();
        contour.push(contour[0]);
        self
    }
    fn polygon(&mut self, points: &[(f64, f64)]) -> &mut Self {
        self.m(points[0].0, points[0].1);
        for &(x, y) in &points[1..] {
            self.l(x, y);
        }
        self.z()
    }
    fn rect(&mut self, x: f64, y: f64, w: f64, h: f64) -> &mut Self {
        self.polygon(&[(x, y), (x + w, y), (x + w, y + h), (x, y + h)])
    }
    fn ellipse(&mut self, x: f64, y: f64, rx: f64, ry: f64) -> &mut Self {
        let k = 0.5522847498307936;
        self.m(x + rx, y)
            .c(x + rx, y + ry * k, x + rx * k, y + ry, x, y + ry)
            .c(x - rx * k, y + ry, x - rx, y + ry * k, x - rx, y)
            .c(x - rx, y - ry * k, x - rx * k, y - ry, x, y - ry)
            .c(x + rx * k, y - ry, x + rx, y - ry * k, x + rx, y)
            .z()
    }
    fn transform(&mut self, f: impl Fn(Point) -> Point) {
        for command in &mut self.commands {
            match command {
                Command::Move(a) | Command::Line(a) => *a = f(*a),
                Command::Quad(a, b) => {
                    *a = f(*a);
                    *b = f(*b);
                }
                Command::Cubic(a, b, c) => {
                    *a = f(*a);
                    *b = f(*b);
                    *c = f(*c);
                }
                Command::Close => {}
            }
        }
        for contour in &mut self.contours {
            for p in contour {
                *p = f(*p);
            }
        }
    }
    pub fn svg(&self, r: Rect) -> String {
        let map = |p: Point| Point::new(r.x + p.x * r.w / 100., r.y + p.y * r.h / 100.);
        let mut s = String::new();
        for command in &self.commands {
            match *command {
                Command::Move(a) | Command::Line(a) => {
                    let p = map(a);
                    let op = if matches!(command, Command::Move(_)) {
                        'M'
                    } else {
                        'L'
                    };
                    let _ = write!(s, "{op}{} {} ", p.x, p.y);
                }
                Command::Quad(a, b) => {
                    let a = map(a);
                    let b = map(b);
                    let _ = write!(s, "Q{} {} {} {} ", a.x, a.y, b.x, b.y);
                }
                Command::Cubic(a, b, c) => {
                    let a = map(a);
                    let b = map(b);
                    let c = map(c);
                    let _ = write!(s, "C{} {} {} {} {} {} ", a.x, a.y, b.x, b.y, c.x, c.y);
                }
                Command::Close => s.push_str("Z "),
            }
        }
        s
    }
}
pub struct Geometry {
    pub outline: Path,
    pub detail: Path,
    text: Rect,
}
impl Geometry {
    pub fn text_rect(&self, r: Rect) -> Rect {
        Rect {
            x: r.x + self.text.x * r.w / 100.,
            y: r.y + self.text.y * r.h / 100.,
            w: self.text.w * r.w / 100.,
            h: self.text.h * r.h / 100.,
        }
    }
    pub fn contains(&self, p: Point) -> bool {
        let mut inside = false;
        for contour in &self.outline.contours {
            for pair in contour.windows(2) {
                let (a, b) = (pair[0], pair[1]);
                if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x
                {
                    inside = !inside;
                }
            }
        }
        inside
    }
    pub fn near_outline(&self, r: Rect, p: Point, tolerance: f64) -> bool {
        self.outline.contours.iter().any(|c| {
            c.windows(2).any(|s| {
                let map = |p: Point| Point::new(r.x + p.x * r.w / 100., r.y + p.y * r.h / 100.);
                geometry::distance_to_segment(p, map(s[0]), map(s[1])) <= tolerance
            })
        })
    }
    pub fn port(&self, side: u8, t: f64) -> Point {
        let edge = match side {
            0 => Point::new(t * 100., 0.),
            1 => Point::new(100., t * 100.),
            2 => Point::new(t * 100., 100.),
            _ => Point::new(0., t * 100.),
        };
        let origin = Point::new(50., 50.);
        let ray = Point::new(edge.x - 50., edge.y - 50.);
        let mut best = None::<(f64, Point)>;
        let mut nearest = (f64::INFINITY, edge);
        for c in &self.outline.contours {
            for pair in c.windows(2) {
                let (a, b) = (pair[0], pair[1]);
                let delta = Point::new(b.x - a.x, b.y - a.y);
                let cross = |a: Point, b: Point| a.x * b.y - a.y * b.x;
                let den = cross(ray, delta);
                if den.abs() > 1e-10 {
                    let offset = Point::new(a.x - origin.x, a.y - origin.y);
                    let u = cross(offset, delta) / den;
                    let v = cross(offset, ray) / den;
                    if u >= 0.
                        && (-1e-9..=1. + 1e-9).contains(&v)
                        && best.is_none_or(|(old, _)| u > old)
                    {
                        best = Some((u, Point::new(origin.x + u * ray.x, origin.y + u * ray.y)));
                    }
                }
                let length = delta.x * delta.x + delta.y * delta.y;
                if length > 0. {
                    let t = (((edge.x - a.x) * delta.x + (edge.y - a.y) * delta.y) / length)
                        .clamp(0., 1.);
                    let p = Point::new(a.x + t * delta.x, a.y + t * delta.y);
                    let distance = p.distance(edge);
                    if distance < nearest.0 {
                        nearest = (distance, p);
                    }
                }
            }
        }
        best.map_or(nearest.1, |(_, p)| p)
    }
}
pub fn geometry(shape: Shape) -> Option<&'static Geometry> {
    if !shape.is_extended() {
        return None;
    }
    static ALL: OnceLock<HashMap<Shape, Geometry>> = OnceLock::new();
    ALL.get_or_init(|| {
        SHAPES
            .iter()
            .filter(|(s, _)| s.is_extended())
            .map(|(s, _)| (*s, build(*s)))
            .collect()
    })
    .get(&shape)
}
fn build(shape: Shape) -> Geometry {
    use Shape::*;
    let mut p = Path::default();
    let mut detail = Path::default();
    let mut text = Rect {
        x: 15.,
        y: 15.,
        w: 70.,
        h: 70.,
    };
    match shape {
        RightTriangle => {
            p.polygon(&[(0., 0.), (100., 100.), (0., 100.)]);
            text = Rect {
                x: 5.,
                y: 60.,
                w: 50.,
                h: 30.,
            };
        }
        Parallelogram => {
            p.polygon(&[(22., 0.), (100., 0.), (78., 100.), (0., 100.)]);
        }
        Trapezoid => {
            p.polygon(&[(22., 0.), (78., 0.), (100., 100.), (0., 100.)]);
        }
        Pentagon | Hexagon | Octagon | Star4 | Star5 | Star6 | Star8 => {
            let (n, inner) = match shape {
                Pentagon => (5, None),
                Hexagon => (6, None),
                Octagon => (8, None),
                Star4 => (4, Some(0.27)),
                Star5 => (5, Some(0.44)),
                Star6 => (6, Some(0.55)),
                _ => (8, Some(0.65)),
            };
            let count = if inner.is_some() { n * 2 } else { n };
            let points: Vec<_> = (0..count)
                .map(|i| {
                    let a = -std::f64::consts::FRAC_PI_2
                        + i as f64 * std::f64::consts::TAU / count as f64;
                    let r = if i % 2 == 1 { inner.unwrap_or(1.) } else { 1. };
                    (50. + 50. * r * a.cos(), 50. + 50. * r * a.sin())
                })
                .collect();
            p.polygon(&points);
            if inner.is_some() {
                text = Rect {
                    x: 30.,
                    y: 30.,
                    w: 40.,
                    h: 40.,
                };
            }
        }
        Heart => {
            p.m(50., 20.)
                .c(25., 0., 0., 0., 0., 28.)
                .c(0., 48., 24., 70., 50., 100.)
                .c(76., 70., 100., 48., 100., 28.)
                .c(100., 0., 75., 0., 50., 20.)
                .z();
            text = Rect {
                x: 20.,
                y: 25.,
                w: 60.,
                h: 40.,
            };
        }
        Moon => {
            p.m(100., 0.)
                .c(50., 18., 50., 82., 100., 100.)
                .c(-33., 100., -33., 0., 100., 0.)
                .z();
            text = Rect {
                x: 10.,
                y: 30.,
                w: 40.,
                h: 40.,
            };
        }
        Sun => {
            p.ellipse(50., 50., 28., 28.);
            for i in 0..8 {
                let a = i as f64 * std::f64::consts::TAU / 8.;
                let points = [(38., a - 0.12), (50., a), (38., a + 0.12)]
                    .map(|(r, a)| (50. + r * a.cos(), 50. + r * a.sin()));
                p.polygon(&points);
            }
            text = Rect {
                x: 30.,
                y: 30.,
                w: 40.,
                h: 40.,
            };
        }
        Lightning => {
            p.polygon(&[
                (44., 0.),
                (88., 0.),
                (59., 39.),
                (96., 39.),
                (21., 100.),
                (39., 57.),
                (4., 57.),
            ]);
        }
        Donut => {
            p.ellipse(50., 50., 50., 50.).ellipse(50., 50., 28., 28.);
        }
        Plus => {
            p.polygon(&[
                (35., 0.),
                (65., 0.),
                (65., 35.),
                (100., 35.),
                (100., 65.),
                (65., 65.),
                (65., 100.),
                (35., 100.),
                (35., 65.),
                (0., 65.),
                (0., 35.),
                (35., 35.),
            ]);
        }
        Minus => {
            p.rect(0., 35., 100., 30.);
        }
        Multiply => {
            p.polygon(&[
                (0., 20.),
                (20., 0.),
                (50., 30.),
                (80., 0.),
                (100., 20.),
                (70., 50.),
                (100., 80.),
                (80., 100.),
                (50., 70.),
                (20., 100.),
                (0., 80.),
                (30., 50.),
            ]);
        }
        Divide => {
            p.rect(0., 39., 100., 22.)
                .ellipse(50., 12., 12., 12.)
                .ellipse(50., 88., 12., 12.);
        }
        Equals => {
            p.rect(0., 20., 100., 20.).rect(0., 60., 100., 20.);
        }
        NotEqual => {
            // One outline avoids even-odd holes where the slash crosses a bar.
            p.polygon(&[
                (66., 0.),
                (84., 0.),
                (73., 20.),
                (100., 20.),
                (100., 40.),
                (62., 40.),
                (51., 60.),
                (100., 60.),
                (100., 80.),
                (40., 80.),
                (29., 100.),
                (11., 100.),
                (22., 80.),
                (0., 80.),
                (0., 60.),
                (33., 60.),
                (44., 40.),
                (0., 40.),
                (0., 20.),
                (55., 20.),
            ]);
        }
        LeftBracket | RightBracket | BracketPair => {
            p.polygon(&[
                (0., 0.),
                (25., 0.),
                (25., 7.),
                (7., 7.),
                (7., 93.),
                (25., 93.),
                (25., 100.),
                (0., 100.),
            ]);
            if shape == LeftBracket {
                p.transform(|p| Point::new(p.x * 4., p.y));
            }
            if shape == RightBracket {
                p.transform(|p| Point::new(100. - p.x * 4., p.y));
            }
            if shape == BracketPair {
                p.polygon(&[
                    (100., 0.),
                    (75., 0.),
                    (75., 7.),
                    (93., 7.),
                    (93., 93.),
                    (75., 93.),
                    (75., 100.),
                    (100., 100.),
                ]);
            }
        }
        LeftBrace | RightBrace | BracePair => {
            p.m(27., 0.)
                .l(27., 7.)
                .q(13., 7., 13., 21.)
                .l(13., 35.)
                .q(13., 46., 7., 50.)
                .q(13., 54., 13., 65.)
                .l(13., 79.)
                .q(13., 93., 27., 93.)
                .l(27., 100.)
                .q(5., 100., 5., 79.)
                .l(5., 65.)
                .q(5., 54., 0., 54.)
                .l(0., 46.)
                .q(5., 46., 5., 35.)
                .l(5., 21.)
                .q(5., 0., 27., 0.)
                .z();
            if shape == LeftBrace {
                p.transform(|p| Point::new(p.x * 100. / 27., p.y));
            }
            if shape == RightBrace {
                p.transform(|p| Point::new(100. - p.x * 100. / 27., p.y));
            }
            if shape == BracePair {
                let mut right = Path {
                    commands: p.commands.clone(),
                    contours: p.contours.clone(),
                };
                right.transform(|p| Point::new(100. - p.x, p.y));
                p.commands.extend(right.commands);
                p.contours.extend(right.contours);
            }
        }
        ArrowRight | ArrowLeft | ArrowUp | ArrowDown => {
            p.polygon(&[
                (0., 32.),
                (65., 32.),
                (65., 0.),
                (100., 50.),
                (65., 100.),
                (65., 68.),
                (0., 68.),
            ]);
            text = Rect {
                x: 4.,
                y: 34.,
                w: 59.,
                h: 32.,
            };
            match shape {
                ArrowLeft => {
                    p.transform(|p| Point::new(100. - p.x, p.y));
                    text.x = 37.;
                }
                ArrowUp => {
                    p.transform(|p| Point::new(p.y, 100. - p.x));
                    text = Rect {
                        x: 34.,
                        y: 37.,
                        w: 32.,
                        h: 59.,
                    };
                }
                ArrowDown => {
                    p.transform(|p| Point::new(p.y, p.x));
                    text = Rect {
                        x: 34.,
                        y: 4.,
                        w: 32.,
                        h: 59.,
                    };
                }
                _ => {}
            }
        }
        ArrowBoth | ArrowUpDown => {
            p.polygon(&[
                (0., 50.),
                (27., 0.),
                (27., 32.),
                (73., 32.),
                (73., 0.),
                (100., 50.),
                (73., 100.),
                (73., 68.),
                (27., 68.),
                (27., 100.),
            ]);
            text = Rect {
                x: 28.,
                y: 34.,
                w: 44.,
                h: 32.,
            };
            if shape == ArrowUpDown {
                p.transform(|p| Point::new(p.y, p.x));
                text = Rect {
                    x: 34.,
                    y: 28.,
                    w: 32.,
                    h: 44.,
                };
            }
        }
        ArrowFourWay => {
            p.polygon(&[
                (50., 0.),
                (72., 25.),
                (60., 25.),
                (60., 40.),
                (75., 40.),
                (75., 28.),
                (100., 50.),
                (75., 72.),
                (75., 60.),
                (60., 60.),
                (60., 75.),
                (72., 75.),
                (50., 100.),
                (28., 75.),
                (40., 75.),
                (40., 60.),
                (25., 60.),
                (25., 72.),
                (0., 50.),
                (25., 28.),
                (25., 40.),
                (40., 40.),
                (40., 25.),
                (28., 25.),
            ]);
            text = Rect {
                x: 40.,
                y: 40.,
                w: 20.,
                h: 20.,
            };
        }
        ArrowBent => {
            p.polygon(&[
                (0., 100.),
                (0., 35.),
                (65., 35.),
                (65., 0.),
                (100., 50.),
                (65., 100.),
                (65., 65.),
                (30., 65.),
                (30., 100.),
            ]);
        }
        ArrowUTurn => {
            p.m(0., 100.)
                .l(0., 35.)
                .q(0., 0., 35., 0.)
                .l(55., 0.)
                .q(90., 0., 90., 35.)
                .l(90., 58.)
                .l(100., 58.)
                .l(75., 100.)
                .l(50., 58.)
                .l(60., 58.)
                .l(60., 35.)
                .q(60., 28., 53., 28.)
                .l(37., 28.)
                .q(30., 28., 30., 35.)
                .l(30., 100.)
                .z();
        }
        Chevron => {
            p.polygon(&[
                (0., 0.),
                (65., 0.),
                (100., 50.),
                (65., 100.),
                (0., 100.),
                (35., 50.),
            ]);
            text = Rect {
                x: 39.,
                y: 25.,
                w: 33.,
                h: 50.,
            };
        }
        CalloutRect => {
            p.polygon(&[
                (0., 0.),
                (100., 0.),
                (100., 73.),
                (49., 73.),
                (15., 100.),
                (25., 73.),
                (0., 73.),
            ]);
            text = Rect {
                x: 8.,
                y: 8.,
                w: 84.,
                h: 57.,
            };
        }
        CalloutRounded => {
            p.m(12., 0.)
                .l(88., 0.)
                .q(100., 0., 100., 12.)
                .l(100., 61.)
                .q(100., 73., 88., 73.)
                .l(49., 73.)
                .l(15., 100.)
                .l(25., 73.)
                .l(12., 73.)
                .q(0., 73., 0., 61.)
                .l(0., 12.)
                .q(0., 0., 12., 0.)
                .z();
            text = Rect {
                x: 10.,
                y: 8.,
                w: 80.,
                h: 57.,
            };
        }
        CalloutOval => {
            p.m(50., 0.)
                .c(78., 0., 100., 17., 100., 38.)
                .q(100., 56., 81., 67.)
                .l(95., 100.)
                .l(60., 75.)
                .c(25., 81., 0., 63., 0., 38.)
                .c(0., 17., 22., 0., 50., 0.)
                .z();
            text = Rect {
                x: 17.,
                y: 12.,
                w: 66.,
                h: 49.,
            };
        }
        CalloutCloud | CalloutThought => {
            p.m(16., 57.)
                .c(0., 64., -5., 34., 12., 31.)
                .c(0., 9., 25., 0., 35., 12.)
                .c(47., -3., 72., -2., 76., 16.)
                .c(97., 5., 110., 30., 94., 39.)
                .c(111., 61., 82., 80., 70., 65.);
            if shape == CalloutCloud {
                p.l(21., 100.).l(32., 65.);
            } else {
                p.q(46., 83., 32., 65.);
            }
            p.q(21., 74., 16., 57.).z();
            if shape == CalloutThought {
                p.ellipse(23., 82., 8., 7.).ellipse(12., 96., 4., 4.);
            }
            text = Rect {
                x: 20.,
                y: 24.,
                w: 60.,
                h: 31.,
            };
        }
        FlowProcessDouble => {
            p.rect(0., 0., 100., 100.);
            detail.m(13., 0.).l(13., 100.).m(87., 0.).l(87., 100.);
        }
        FlowDocument => {
            p.m(0., 0.)
                .l(100., 0.)
                .l(100., 80.)
                .c(65., 55., 35., 105., 0., 80.)
                .z();
            text = Rect {
                x: 10.,
                y: 10.,
                w: 80.,
                h: 54.,
            };
        }
        FlowMultiDocument => {
            p.m(0., 16.)
                .l(8., 16.)
                .l(8., 8.)
                .l(16., 8.)
                .l(16., 0.)
                .l(100., 0.)
                .l(100., 73.)
                .l(92., 73.)
                .l(92., 81.)
                .l(84., 81.)
                .c(55., 60., 29., 108., 0., 88.)
                .z();
            detail
                .m(8., 16.)
                .l(84., 16.)
                .l(84., 81.)
                .m(16., 8.)
                .l(92., 8.)
                .l(92., 73.);
            text = Rect {
                x: 8.,
                y: 25.,
                w: 68.,
                h: 42.,
            };
        }
        FlowTerminator => {
            p.m(25., 0.)
                .l(75., 0.)
                .c(108., 0., 108., 100., 75., 100.)
                .l(25., 100.)
                .c(-8., 100., -8., 0., 25., 0.)
                .z();
        }
        FlowManualInput => {
            p.polygon(&[(0., 25.), (100., 0.), (100., 100.), (0., 100.)]);
            text = Rect {
                x: 10.,
                y: 30.,
                w: 80.,
                h: 55.,
            };
        }
        FlowPreparation => {
            p.polygon(&[
                (20., 0.),
                (80., 0.),
                (100., 50.),
                (80., 100.),
                (20., 100.),
                (0., 50.),
            ]);
        }
        FlowOffpage => {
            p.polygon(&[(0., 0.), (100., 0.), (100., 65.), (50., 100.), (0., 65.)]);
            text = Rect {
                x: 10.,
                y: 10.,
                w: 80.,
                h: 50.,
            };
        }
        FlowDelay => {
            p.m(0., 0.)
                .l(50., 0.)
                .c(116.66666666666666, 0., 116.66666666666666, 100., 50., 100.)
                .l(0., 100.)
                .z();
        }
        _ => unreachable!("legacy shapes use their unchanged renderer"),
    }
    Geometry {
        outline: p,
        detail,
        text,
    }
}
