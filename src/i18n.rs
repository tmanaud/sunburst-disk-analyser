//! Interface translations. English text is the lookup key.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Lang {
    En = 0,
    Fr,
    De,
    Es,
    It,
    Ru,
    Uk,
    Zh,
}

/// Language codes with their native names, in menu order.
pub const LANGUAGES: &[(&str, &str)] = &[
    ("en", "English"),
    ("fr", "Français"),
    ("de", "Deutsch"),
    ("es", "Español"),
    ("it", "Italiano"),
    ("ru", "Русский"),
    ("uk", "Українська"),
    ("zh", "简体中文"),
];

static CURRENT: AtomicU8 = AtomicU8::new(0);

fn from_code(code: &str) -> Option<Lang> {
    let code = code.to_lowercase();
    Some(match code.split(['-', '_']).next()? {
        "en" => Lang::En,
        "fr" => Lang::Fr,
        "de" => Lang::De,
        "es" => Lang::Es,
        "it" => Lang::It,
        "ru" => Lang::Ru,
        "uk" => Lang::Uk,
        "zh" => Lang::Zh,
        _ => return None,
    })
}

/// Turns a setting ("auto" or a language code) into a language.
pub fn resolve(setting: &str) -> Lang {
    from_code(setting)
        .or_else(|| sys_locale::get_locale().and_then(|l| from_code(&l)))
        .unwrap_or(Lang::En)
}

pub fn set_lang(lang: Lang) {
    CURRENT.store(lang as u8, Ordering::Relaxed);
}

pub fn lang() -> Lang {
    match CURRENT.load(Ordering::Relaxed) {
        1 => Lang::Fr,
        2 => Lang::De,
        3 => Lang::Es,
        4 => Lang::It,
        5 => Lang::Ru,
        6 => Lang::Uk,
        7 => Lang::Zh,
        _ => Lang::En,
    }
}

pub fn date_format() -> &'static str {
    match lang() {
        Lang::En | Lang::Fr | Lang::Es | Lang::It => "%d/%m/%Y",
        Lang::De | Lang::Ru | Lang::Uk => "%d.%m.%Y",
        Lang::Zh => "%Y/%m/%d",
    }
}

fn table() -> &'static HashMap<&'static str, [&'static str; 7]> {
    static TABLE: OnceLock<HashMap<&'static str, [&'static str; 7]>> = OnceLock::new();
    TABLE.get_or_init(|| crate::i18n::strings::STRINGS.iter().map(|(k, v)| (*k, *v)).collect())
}

/// Translates an English interface string to the current language.
pub fn t(key: &'static str) -> &'static str {
    let l = lang();
    if l == Lang::En {
        return key;
    }
    match table().get(key) {
        Some(row) if !row[l as usize - 1].is_empty() => row[l as usize - 1],
        _ => key,
    }
}

/// Translates and fills each `{}` with the next argument.
pub fn tf(key: &'static str, args: &[&str]) -> String {
    let mut out = String::new();
    let mut rest = t(key);
    for a in args {
        match rest.find("{}") {
            Some(i) => {
                out.push_str(&rest[..i]);
                out.push_str(a);
                rest = &rest[i + 2..];
            }
            None => break,
        }
    }
    out.push_str(rest);
    out
}

/// `tf` for a string that starts with a count: fixes the English singular.
pub fn tn(key: &'static str, count: u64, args: &[&str]) -> String {
    let s = tf(key, args);
    if count == 1 && lang() == Lang::En {
        s.replacen(" items", " item", 1).replacen(" files", " file", 1).replacen(" folders", " folder", 1)
    } else {
        s
    }
}

#[path = "strings.rs"]
pub mod strings;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_survive_translation() {
        for (key, row) in strings::STRINGS {
            let n = key.matches("{}").count();
            for (i, text) in row.iter().enumerate() {
                assert!(!text.is_empty(), "missing translation {i} for {key:?}");
                assert_eq!(text.matches("{}").count(), n, "placeholder mismatch in translation {i} of {key:?}");
            }
        }
    }

    #[test]
    fn codes_resolve() {
        assert_eq!(resolve("fr"), Lang::Fr);
        assert_eq!(from_code("zh-Hans-CN"), Some(Lang::Zh));
        assert_eq!(from_code("pt_BR"), None);
        set_lang(Lang::En);
        assert_eq!(tf("{} items", &["3"]), "3 items");
    }
}
