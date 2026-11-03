//! 界面语言的检测与匹配。

/// 把 BCP-47 语言标记（`zh-Hans-CN`、`en_US` 之类）解析成 `available` 中实际存在的语言。
///
/// 匹配顺序：完全一致 → 逐段截断（`zh-Hans-CN` → `zh-Hans` → `zh`）→ 同语言的区域/书写系统近似匹配。
pub fn resolve_locale(tag: &str, available: &[String]) -> Option<String> {
    let tag = tag.replace('_', "-").to_ascii_lowercase();
    if tag.is_empty() {
        return None;
    }

    if let Some(hit) = available.iter().find(|l| l.eq_ignore_ascii_case(&tag)) {
        return Some(hit.clone());
    }

    let mut end = tag.len();
    while let Some(index) = tag[..end].rfind('-') {
        end = index;
        if let Some(hit) = available
            .iter()
            .find(|l| l.eq_ignore_ascii_case(&tag[..end]))
        {
            return Some(hit.clone());
        }
    }

    let (language, hints) = match tag.split_once('-') {
        Some((language, rest)) => (language, rest.split('-').collect::<Vec<_>>()),
        None => (tag.as_str(), Vec::new()),
    };
    let mut first_of_language = None;
    for candidate in available {
        let lowered = candidate.to_ascii_lowercase();
        let Some((candidate_language, candidate_rest)) = lowered.split_once('-') else {
            continue;
        };
        if candidate_language != language {
            continue;
        }
        first_of_language.get_or_insert_with(|| candidate.clone());

        let segments: Vec<&str> = candidate_rest.split('-').collect();
        // 区域一致，或书写系统能对上（zh-Hans-* 更愿意用 zh-CN，zh-Hant-* 更愿意用 zh-TW/zh-HK）
        let region_matches = hints.iter().any(|hint| segments.contains(hint));
        let script_matches = if hints.contains(&"hans") {
            segments.contains(&"cn") || segments.contains(&"sg")
        } else if hints.contains(&"hant") {
            segments.contains(&"tw") || segments.contains(&"hk")
        } else {
            false
        };
        if region_matches || script_matches {
            return Some(candidate.clone());
        }
    }
    first_of_language
}

/// 取系统首选语言列表中第一个受支持的语言，都匹配不上时用 `fallback`。
///
/// `available` 由调用方给出（如 `rust_i18n::available_locales!()` 的结果）：
/// 该宏展开为 `crate::_rust_i18n_available_locales()`，只能在初始化了 `rust_i18n::i18n!` 的 crate 里使用。
pub fn detect_locale(available: &[String], fallback: &str) -> String {
    sys_locale::get_locales()
        .find_map(|tag| resolve_locale(&tag, available))
        .unwrap_or_else(|| fallback.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_locale_test() {
        let available = vec![
            "en".to_string(),
            "zh-Hans".to_string(),
            "zh-Hant".to_string(),
            "zh-CN".to_string(),
            "zh-TW".to_string(),
            "zh-HK".to_string(),
        ];
        assert_eq!(resolve_locale("en", &available), Some("en".to_string()));
        assert_eq!(resolve_locale("en-US", &available), Some("en".to_string()));
        assert_eq!(
            resolve_locale("zh-Hans-CN", &available),
            Some("zh-Hans".to_string())
        );
        assert_eq!(
            resolve_locale("zh-Hant-TW", &available),
            Some("zh-Hant".to_string())
        );
        assert_eq!(
            resolve_locale("zh-Hant-HK", &available),
            Some("zh-Hant".to_string())
        );
        assert_eq!(
            resolve_locale("zh-Hant-MO", &available),
            Some("zh-Hant".to_string())
        );
        assert_eq!(
            resolve_locale("zh-SG", &available),
            Some("zh-Hans".to_string())
        );
    }

    #[test]
    fn detect_locale_use_first_match() {
        let available = vec![
            "en".to_string(),
            "zh-Hans".to_string(),
            "zh-Hant".to_string(),
            "zh-CN".to_string(),
            "zh-TW".to_string(),
            "zh-HK".to_string(),
        ];
        assert_eq!(detect_locale(&available, "en"), "en".to_string());
    }

    #[test]
    fn detect_unknown_locale_should_fallback() {
        let unknown_locale = ["unknown".to_string()];
        assert_eq!(
            detect_locale(&unknown_locale, "en"),
            "en".to_string()
        );
    }
}
