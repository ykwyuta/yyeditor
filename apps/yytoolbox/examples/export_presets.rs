//! Refresh the checked-in dummy CSV definitions from the examples shown by the UI.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../demo/yytoolbox");
    std::fs::create_dir_all(&folder)?;
    for (name, filename) in [
        ("個人", "person"),
        ("企業", "company"),
        ("EC注文", "orders"),
        ("医療", "medical"),
        ("金融", "finance"),
        ("物流", "logistics"),
        ("ITログ", "it-logs"),
        ("IoT", "iot"),
    ] {
        let mut params = yytoolbox::Tool::DummyCsv.defaults();
        params.insert("preset".into(), name.into());
        let definition = yytoolbox::Tool::DummyCsv.example(&params);
        std::fs::write(folder.join(format!("{filename}.yaml")), definition)?;
    }
    Ok(())
}
