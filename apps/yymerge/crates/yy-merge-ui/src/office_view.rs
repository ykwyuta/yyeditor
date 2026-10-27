//! Native regression coverage for read-only Office review, using the normal workers.
use super::*;

pub(super) fn smoke(a: &mut App, dir: &std::path::Path) -> Result<()> {
    let original = a.options.clone();
    for (ext, left, right, expected) in [
        (
            "xlsx",
            include_bytes!("../../../../../demo/yymerge/office/left.xlsx").as_slice(),
            include_bytes!("../../../../../demo/yymerge/office/right.xlsx").as_slice(),
            "CELL",
        ),
        (
            "pptx",
            include_bytes!("../../../../../demo/yymerge/office/left.pptx").as_slice(),
            include_bytes!("../../../../../demo/yymerge/office/right.pptx").as_slice(),
            "SLIDE",
        ),
    ] {
        let l = dir.join(format!("office-left.{ext}"));
        let r = dir.join(format!("office-right.{ext}"));
        std::fs::write(&l, left).map_err(|e| e.to_string())?;
        std::fs::write(&r, right).map_err(|e| e.to_string())?;
        a.options = Gui {
            left: Some(l.clone()),
            right: Some(r.clone()),
            ..Default::default()
        };
        begin(a);
        await_rule_smoke(a)?;
        if a.editors.len() != 2
            || a.exit != 1
            || !a.editors[0].text().contains(expected)
            || a.loaded.as_ref().unwrap().merged.is_some()
            || replace_view::command(a, REPLACE_OPEN).is_ok()
        {
            return Err("Office review lost projection/difference/read-only mode".into());
        }
        let c = Cancellation::new();
        let data = a.loaded.as_ref().unwrap();
        let session = yy_merge_core::comparison_session::Session::capture(
            [&data.inputs[0], &data.inputs[1]],
            data.rules,
            Default::default(),
            &c,
        )?;
        let session_path = dir.join(format!("office-{ext}.json"));
        std::fs::write(&session_path, session.json(&c)?).map_err(|e| e.to_string())?;
        a.options = Gui {
            comparison_session: Some(session_path),
            ..Default::default()
        };
        begin(a);
        await_rule_smoke(a)?;
        if a.editors.len() != 2 || !a.editors[0].text().contains(expected) || a.exit != 1 {
            return Err("Office comparison session restore failed".into());
        }
        let path = dir.join(format!("office-{ext}.html"));
        report_view::start(a, path.clone())?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(result) = report_view::tick(a) {
                result?;
                break;
            }
            if Instant::now() > deadline {
                return Err("Office HTML timeout".into());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let html = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        if !html.contains("Office抽出データ") || !html.contains(expected) {
            return Err("Office HTML lost extracted content notice".into());
        }
        let mut merge = a.options.clone();
        merge.base = Some(l.clone());
        if load(&merge, &c).is_ok() {
            return Err("Office projection was accepted for merge".into());
        }
        if std::fs::read(&l).map_err(|e| e.to_string())? != left
            || std::fs::read(&r).map_err(|e| e.to_string())? != right
        {
            return Err("Office source changed".into());
        }
    }
    a.options = original;
    begin(a);
    await_rule_smoke(a)?;
    std::fs::write(dir.join("office-smoke.txt"), "PASS: Excel/PowerPoint automatic read-only extraction, two-pane difference display, comparison session restore, background HTML, replacement/merge rejected, original package bytes retained\n").map_err(|e| e.to_string())
}
