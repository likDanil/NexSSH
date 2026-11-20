use crate::{AppWindow, I18n};
use slint::ComponentHandle;
include!(concat!(env!("OUT_DIR"), "/locales.rs"));

pub fn resolve(language: &str) -> &'static str {
    let system = sys_locale::get_locale().unwrap_or_else(|| "en".into());
    let language = if language == "system" {
        &system
    } else {
        language
    };
    match nexssh_core::i18n::Lang::from_tag(language) {
        Some(nexssh_core::i18n::Lang::Ru) => "ru",
        _ => "en",
    }
}

pub fn tr(key: &str, language: &str) -> String {
    if let Ok(index) = TRANSLATIONS.binary_search_by_key(&key, |row| row.0) {
        let (_, en, ru) = TRANSLATIONS[index];
        return if language == "ru" { ru } else { en }.into();
    }
    key.into()
}

pub fn init(ui: &AppWindow, language: &str) {
    ui.global::<I18n>()
        .on_translate(|key, language| tr(&key, &language).into());
    apply(ui, language);
}

pub fn apply(ui: &AppWindow, language: &str) {
    let language = resolve(language);
    ui.global::<I18n>().set_language(language.into());
    nexssh_core::i18n::set_language(nexssh_core::i18n::Lang::from_tag(language).unwrap());
}
