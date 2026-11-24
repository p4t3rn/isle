// SPDX-License-Identifier: GPL-3.0-or-later

#[derive(Clone, Copy)]
pub enum Locale {
    English,
    SimplifiedChinese,
}

impl Locale {
    pub fn messages() -> Self {
        let primary = ["LC_ALL", "LC_MESSAGES", "LANG"]
            .into_iter()
            .find_map(|key| std::env::var(key).ok().filter(|value| !value.is_empty()));
        let languages = std::env::var("LANGUAGE").unwrap_or_default();
        Self::select(primary.as_deref().unwrap_or("C"), &languages)
    }

    fn select(primary: &str, languages: &str) -> Self {
        let primary = primary.split(['.', '@']).next().unwrap_or("");
        if matches!(primary, "C" | "POSIX") {
            return Self::English;
        }
        for candidate in languages.split(':').chain(std::iter::once(primary)) {
            let candidate = candidate.split(['.', '@']).next().unwrap_or("");
            let normalized = candidate.replace('-', "_").to_ascii_lowercase();
            if matches!(
                normalized.as_str(),
                "zh" | "zh_cn" | "zh_sg" | "zh_hans" | "zh_hans_cn" | "zh_hans_sg"
            ) {
                return Self::SimplifiedChinese;
            }
            if normalized == "en" || normalized.starts_with("en_") {
                return Self::English;
            }
        }
        Self::English
    }

    pub fn is_chinese(self) -> bool {
        matches!(self, Self::SimplifiedChinese)
    }

    pub fn choose(self, english: &'static str, chinese: &'static str) -> &'static str {
        if self.is_chinese() { chinese } else { english }
    }
}

#[cfg(test)]
mod tests {
    use super::Locale;

    #[test]
    fn simplified_chinese_respects_locale_and_language_priority() {
        assert!(Locale::select("zh_CN.UTF-8", "").is_chinese());
        assert!(Locale::select("fr_FR.UTF-8", "fr:zh-Hans").is_chinese());
        assert!(!Locale::select("zh_TW.UTF-8", "").is_chinese());
        assert!(!Locale::select("zh_CN.UTF-8", "en_US:zh_CN").is_chinese());
        assert!(!Locale::select("C.UTF-8", "zh_CN").is_chinese());
    }
}
