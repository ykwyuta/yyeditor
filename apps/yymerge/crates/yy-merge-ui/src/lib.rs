//! Windows host with the yy series' shared editor, font and offline help.
#[cfg(windows)]
mod comparison;
#[cfg(windows)]
mod image_view;
#[cfg(windows)]
mod native;
#[cfg(windows)]
pub fn run(g: yy_merge_cli::Gui) -> yy_merge_core::Result<i32> {
    if g.image
        || [&g.left, &g.right]
            .into_iter()
            .flatten()
            .any(|p| yy_merge_core::images::is_image(p))
    {
        image_view::run(g)
    } else {
        native::run(g)
    }
}
