//! Discover terminal fonts off the UI thread. Only family names survive enumeration.
pub const BUNDLED: &str = "JetBrains Mono";

pub fn discover() -> Vec<String> {
    let mut database = fontdb::Database::new();
    database.load_system_fonts();
    let mut names = std::collections::BTreeMap::new();
    for face in database.faces().filter(|face| face.monospaced) {
        if let Some((name, _)) = face.families.first()
            && !name.starts_with('@')
            && !name.trim().is_empty()
        {
            names
                .entry(name.to_lowercase())
                .or_insert_with(|| name.clone());
        }
    }
    names.into_values().collect()
}

/// Preserve 1.x names and quoted CSS stacks, without a proportional UI fallback.
pub fn resolve<'a>(families: Option<&'a [String]>, requested: &'a str) -> &'a str {
    for name in requested
        .split(',')
        .map(|s| s.trim().trim_matches(['\'', '"']))
    {
        if name.eq_ignore_ascii_case(BUNDLED) {
            return BUNDLED;
        }
        if let Some(families) = families {
            if let Some(installed) = families.iter().find(|s| s.eq_ignore_ascii_case(name)) {
                return installed;
            }
        } else if !name.is_empty() {
            // Discovery has not completed yet.
            return name;
        }
    }
    BUNDLED
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn installed_families_and_legacy_stacks_resolve_without_ui_font_fallback() {
        let families = vec!["Consolas".into(), "Courier New".into()];
        for requested in ["", "missing font", "Arial", "monospace"] {
            assert_eq!(resolve(Some(&families), requested), BUNDLED);
        }
        assert_eq!(resolve(Some(&families), "  consolas "), "Consolas");
        assert_eq!(
            resolve(Some(&families), "'missing', 'Courier New', monospace"),
            "Courier New"
        );
        assert_eq!(resolve(Some(&families), "JetBrains Mono"), BUNDLED);
        assert_eq!(resolve(None, "Consolas"), "Consolas");
    }
}
