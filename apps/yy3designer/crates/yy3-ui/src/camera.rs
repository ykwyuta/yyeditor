use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    Front,
    Back,
    Left,
    Right,
    Top,
    Bottom,
    Isometric,
}
impl Preset {
    pub const ALL: [Self; 7] = [
        Self::Front,
        Self::Back,
        Self::Left,
        Self::Right,
        Self::Top,
        Self::Bottom,
        Self::Isometric,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Front => "正面",
            Self::Back => "背面",
            Self::Left => "左面",
            Self::Right => "右面",
            Self::Top => "上面",
            Self::Bottom => "下面",
            Self::Isometric => "斜め",
        }
    }
    /// Camera looks toward positive camera-space Z; world Y points up.
    pub fn angles(self) -> (f32, f32) {
        match self {
            Self::Front => (0., 0.),
            Self::Back => (PI, 0.),
            Self::Left => (FRAC_PI_2, 0.),
            Self::Right => (-FRAC_PI_2, 0.),
            Self::Top => (0., -FRAC_PI_2),
            Self::Bottom => (0., FRAC_PI_2),
            Self::Isometric => (-FRAC_PI_4, -(1f32 / 2f32.sqrt()).atan()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yy3_core::{Object, mesh};
    #[test]
    fn presets_render_the_named_faces_from_outside_the_object() {
        let mut object = Object::new("カメラ試験", [1, 1, 1]).unwrap();
        object.views[0].as_mut().unwrap().layers[0]
            .pixels
            .fill([255; 4]);
        let mut model = mesh::build(&object).unwrap();
        for t in &mut model.triangles {
            t.color = if t.points.iter().all(|p| p[0] == 0.) {
                [255, 0, 0, 255]
            } else if t.points.iter().all(|p| p[0] == 1.) {
                [0, 255, 0, 255]
            } else if t.points.iter().all(|p| p[1] == 0.) {
                [0, 0, 255, 255]
            } else if t.points.iter().all(|p| p[1] == 1.) {
                [255, 255, 0, 255]
            } else if t.points.iter().all(|p| p[2] == 0.) {
                [255, 0, 255, 255]
            } else {
                [0, 255, 255, 255]
            };
        }
        let masks = [
            [true, false, true],
            [false, true, true],
            [true, false, false],
            [false, true, false],
            [true, true, false],
            [false, false, true],
        ];
        for (preset, expected) in Preset::ALL.into_iter().zip(masks) {
            let (yaw, pitch) = preset.angles();
            let image = mesh::render(&model, 96, 96, yaw, pitch, 1.).unwrap();
            let center = image.pixels[48 * 96 + 48];
            assert_eq!(
                [center[0] > 32, center[1] > 32, center[2] > 32],
                expected,
                "{preset:?}"
            );
        }
        let (yaw, pitch) = Preset::Isometric.angles();
        let image = mesh::render(&model, 96, 96, yaw, pitch, 1.).unwrap();
        for expected in [
            [true, false, true],
            [false, true, false],
            [true, true, false],
        ] {
            assert!(
                image
                    .pixels
                    .iter()
                    .any(|p| [p[0] > 32, p[1] > 32, p[2] > 32] == expected),
                "isometric view should show front, right and top"
            );
        }
    }
}
