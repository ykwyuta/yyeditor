use std::path::PathBuf;
use yy3_core::{image, mesh, *};
fn main() -> std::io::Result<()> {
    let path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("demo/yy3designer"));
    std::fs::create_dir_all(&path)?;
    let mut p = Project::default();
    p.objects[0] = Object::new("ピクセルロボ", [32, 32, 6])?;
    let v = p.object_mut().enable(Direction::Front);
    for (a, b, c) in [
        ([9, 3], [22, 14], [35, 60, 85, 255]),
        ([11, 1], [12, 3], [247, 150, 45, 255]),
        ([19, 1], [20, 3], [247, 150, 45, 255]),
        ([7, 15], [24, 25], [31, 155, 176, 255]),
        ([9, 26], [13, 30], [35, 60, 85, 255]),
        ([18, 26], [22, 30], [35, 60, 85, 255]),
        ([3, 16], [6, 22], [247, 150, 45, 255]),
        ([25, 16], [28, 22], [247, 150, 45, 255]),
        ([11, 6], [14, 9], [103, 232, 249, 255]),
        ([18, 6], [21, 9], [103, 232, 249, 255]),
        ([13, 12], [18, 12], [245, 247, 250, 255]),
        ([11, 18], [20, 21], [245, 247, 250, 255]),
    ] {
        shape(v, Tool::Rectangle, [a, b], c, 1, true, None);
    }
    for d in Direction::ALL.into_iter().skip(1) {
        let view = mesh::projected_view(p.object(), d)?;
        p.object_mut().views[d.index()] = Some(view);
    }
    let mut round = p.object().clone();
    round.name = "角丸ロボ".into();
    round.roundness = 0.7;
    p.objects.push(round);
    save(&path.join("robots.yy3d"), &p)?;
    image::write_png(
        &path.join("robot.png"),
        &image::export_view(p.object().views[0].as_ref().unwrap(), 1, 0.)?,
    )?;
    let m = mesh::build(&p.objects[1])?;
    image::write_png(
        &path.join("preview.png"),
        &mesh::render(&m, 480, 480, -0.5, -0.4, 1.)?,
    )?;
    println!("{}", path.display());
    Ok(())
}
