use std::path::Path;

pub(crate) fn guard(root: &Path) -> oa_sealed::Sealed {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(root.join(".openatca-data-root"), b"openatca-data-root-v1\n").unwrap();
    let text = format!(
        "root {}\nseal scds/rec-20261119\nseal scds/stdds-20261119-holdout\nseal scds/stdds-smoke-20261119*\nseal raw/adsblol/2026.11.20\n",
        root.display()
    );
    oa_sealed::Sealed::with_list(&text, None)
}
