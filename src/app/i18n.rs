#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UiLanguage {
    #[default]
    En,
    ZhCn,
}

impl UiLanguage {
    pub const fn label(self) -> &'static str {
        match self {
            Self::En => "English",
            Self::ZhCn => "简体中文",
        }
    }

    pub const fn app_title(self) -> &'static str {
        match self {
            Self::En => "DG-Lab Audio Link",
            Self::ZhCn => "DG-Lab 音频联动",
        }
    }

    pub const fn all() -> [Self; 2] {
        [Self::En, Self::ZhCn]
    }
}

pub fn detect_system_language() -> UiLanguage {
    let Some(locale) = sys_locale::get_locale() else {
        return UiLanguage::En;
    };

    let locale = locale.to_ascii_lowercase();
    if locale.starts_with("zh") || locale.contains("hans") {
        UiLanguage::ZhCn
    } else {
        UiLanguage::En
    }
}

pub const fn tr<'a>(lang: UiLanguage, en: &'a str, zh_cn: &'a str) -> &'a str {
    match lang {
        UiLanguage::En => en,
        UiLanguage::ZhCn => zh_cn,
    }
}
