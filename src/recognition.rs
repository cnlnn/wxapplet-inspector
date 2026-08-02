use crate::wxapkg::Archive;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum EvidenceFamily {
    GlobalConfig,
    EntryConfig,
    IdentityField,
    Agreement,
    ProgramPhrase,
    PageMap,
    Share,
    Page,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum EvidenceTier {
    Fallback,
    Context,
    Primary,
}

#[derive(Clone, Debug)]
struct NameEvidence {
    value: String,
    source: &'static str,
    family: EvidenceFamily,
    tier: EvidenceTier,
    weight: i32,
}

#[derive(Clone, Debug)]
struct RankedCandidate {
    value: String,
    source: &'static str,
    score: i32,
    tier: EvidenceTier,
    #[cfg(test)]
    family_count: usize,
    #[cfg(test)]
    sources: Vec<&'static str>,
    occurrences: usize,
}

#[derive(Debug)]
pub struct Recognition {
    pub name: String,
    pub source: String,
    pub confidence: u8,
    pub candidates: Vec<String>,
    #[cfg(test)]
    pub diagnostics: Vec<String>,
}

const GENERIC_TITLES: &[&str] = &[
    "WeChat",
    "微信",
    "小程序",
    "测试使用",
    "首页",
    "登录",
    "我的",
    "设置",
    "授权",
    "选择地址",
    "订单详情",
    "购物车",
    "活动详情",
    "高级编辑",
    "投票程序",
    "wechat",
    "会员权益",
    "创建教程",
    "活动信息",
    "活动列表",
    "分享",
    "分享设置",
    "服务通知",
    "服務通知",
    "用户协议",
    "隐私政策",
    "隐私保护指引",
    "服务条款",
    "个人中心",
    "我的收藏",
    "Netscape",
    "Mozilla",
    "Taro",
    "weixin",
    "acs-uni-app",
    "uni-app",
    "uni-app x",
    "production",
    "修改成功",
    "最新使用过的",
    "投票",
    "单页模式",
    "來自微信文件",
    "index",
    "查看活动",
];

const GENERIC_PAGE_PREFIXES: &[&str] = &[
    "选择", "添加", "修改", "重置", "提交", "确认", "编辑", "详情", "列表", "记录", "设置", "管理",
    "登录", "支付", "订单", "活动", "门禁", "房屋", "地址", "信息", "开门", "交易", "关于", "Demo",
    "OPEN ",
];

fn clean_name(value: &str) -> Option<String> {
    let decoded = decode_javascript_escapes(value);
    let mut title = decoded
        .trim()
        .trim_matches(['"', '\'', '“', '”', '‘', '’'])
        .to_owned();
    if let Some(stripped) = title.strip_suffix("小程序") {
        if stripped.chars().count() > 2 {
            title = stripped.to_owned();
        }
    }
    if title.is_empty()
        || title.chars().count() > 48
        || title.chars().any(|ch| "{}[]:;,\"".contains(ch))
        || title.contains('→')
        || title.contains("粉丝参与")
        || title.contains("appName")
        || (title.starts_with('+') && title.ends_with('+'))
        || (title.contains('+') && (title.contains('(') || title.contains(')')))
        || !title
            .chars()
            .any(|ch| ch.is_alphanumeric() || ('\u{4e00}'..='\u{9fff}').contains(&ch))
        || GENERIC_TITLES.contains(&title.as_str())
        || ["领取", "我要", "请拍摄", "该身份证"]
            .iter()
            .any(|prefix| title.starts_with(prefix))
        || title.contains("不存在")
        || looks_like_technical_identifier(&title)
    {
        return None;
    }
    Some(title)
}

fn looks_like_technical_identifier(value: &str) -> bool {
    let has_cjk = value
        .chars()
        .any(|ch| ('\u{4e00}'..='\u{9fff}').contains(&ch));
    if has_cjk || !value.is_ascii() {
        return false;
    }
    let separators = value.chars().filter(|ch| matches!(ch, '-' | '_')).count();
    let lower = value.to_ascii_lowercase();
    separators >= 2
        && ["app", "mobile", "mini", "mp", "weapp", "wechat", "xcx"]
            .iter()
            .any(|token| lower.split(['-', '_']).any(|part| part == *token))
}

fn decode_javascript_escapes(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            output.push(ch);
            continue;
        }
        match chars.next() {
            Some('u') => {
                let hex: String = chars.by_ref().take(4).collect();
                match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                    Some(decoded) => output.push(decoded),
                    None => {
                        output.push_str("\\u");
                        output.push_str(&hex);
                    }
                }
            }
            Some('x') => {
                let hex: String = chars.by_ref().take(2).collect();
                match u8::from_str_radix(&hex, 16).ok() {
                    Some(decoded) => output.push(decoded as char),
                    None => {
                        output.push_str("\\x");
                        output.push_str(&hex);
                    }
                }
            }
            Some('n') => output.push('\n'),
            Some('r') => output.push('\r'),
            Some('t') => output.push('\t'),
            Some(other) => output.push(other),
            None => output.push('\\'),
        }
    }
    output
}

fn add_evidence(
    evidence: &mut Vec<NameEvidence>,
    value: &str,
    source: &'static str,
    family: EvidenceFamily,
    tier: EvidenceTier,
    weight: i32,
) {
    let raw = value.trim();
    if family == EvidenceFamily::Page
        && (GENERIC_PAGE_PREFIXES
            .iter()
            .any(|prefix| raw.starts_with(prefix))
            || ["隐私政策", "用户注册使用协议", "服务协议", "用户协议"]
                .iter()
                .any(|suffix| raw.ends_with(suffix)))
    {
        return;
    }
    let Some(value) = clean_name(raw) else {
        return;
    };
    evidence.push(NameEvidence {
        value,
        source,
        family,
        tier,
        weight,
    });
}

fn quoted_value_after<'a>(text: &'a str, marker: &str, start: usize) -> Option<(&'a str, usize)> {
    let start = text
        .char_indices()
        .find(|(index, _)| *index >= start)
        .map(|(index, _)| index)
        .unwrap_or(text.len());
    let marker_start = text[start..].find(marker)? + start;
    if text[..marker_start].ends_with('.') {
        return None;
    }
    let value_start = marker_start + marker.len();
    let rest = text[value_start..].trim_start();
    let whitespace = text[value_start..].len() - rest.len();
    let quote = rest.chars().next()?;
    if !matches!(quote, '"' | '\'') {
        return None;
    }
    let body_start = value_start + whitespace + quote.len_utf8();
    let body = &text[body_start..];
    let end = body.find(quote)?;
    Some((&body[..end], body_start + end + quote.len_utf8()))
}

fn add_config_evidence(archive: &Archive, evidence: &mut Vec<NameEvidence>) {
    let Some(bytes) = archive.named("app-config.json") else {
        return;
    };
    let Ok(config) = serde_json::from_slice::<Value>(bytes) else {
        return;
    };
    add_evidence(
        evidence,
        config
            .pointer("/global/window/navigationBarTitleText")
            .and_then(Value::as_str)
            .unwrap_or(""),
        "app-config.json:global",
        EvidenceFamily::GlobalConfig,
        EvidenceTier::Primary,
        92,
    );
    if let Some(pages) = config.get("page").and_then(Value::as_object) {
        if let Some(entry_page) = config.get("entryPagePath").and_then(Value::as_str) {
            for key in [entry_page.to_owned(), format!("{entry_page}.html")] {
                if let Some(page) = pages.get(&key) {
                    add_evidence(
                        evidence,
                        page.pointer("/window/navigationBarTitleText")
                            .and_then(Value::as_str)
                            .unwrap_or(""),
                        "app-config.json:entry-page",
                        EvidenceFamily::EntryConfig,
                        EvidenceTier::Context,
                        82,
                    );
                }
            }
        }
        for page in pages.values() {
            add_evidence(
                evidence,
                page.pointer("/window/navigationBarTitleText")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
                "app-config.json:page",
                EvidenceFamily::Page,
                EvidenceTier::Fallback,
                25,
            );
        }
    }
}

fn add_javascript_evidence(text: &str, evidence: &mut Vec<NameEvidence>) {
    for (position, _) in text.match_indices("小程序") {
        let prefix = &text[..position];
        for (opening, closing) in [('“', '”'), ('「', '」'), ('"', '"')] {
            if let Some(open) = prefix.rfind(opening) {
                let value = &prefix[open + opening.len_utf8()..];
                if let Some(end) = value.find(closing) {
                    add_evidence(
                        evidence,
                        value[..end].trim(),
                        "javascript:quoted-program-name",
                        EvidenceFamily::ProgramPhrase,
                        EvidenceTier::Context,
                        72,
                    );
                }
            }
        }
    }
    for (marker, closing) in [("小程序“", '”'), ("小程序「", '」')] {
        let mut start = 0;
        while let Some(found) = text[start..].find(marker) {
            let value_start = start + found + marker.len();
            let value = &text[value_start..];
            let Some(end) = value.find(closing) else {
                break;
            };
            add_evidence(
                evidence,
                &value[..end],
                "javascript:quoted-program-name",
                EvidenceFamily::ProgramPhrase,
                EvidenceTier::Context,
                80,
            );
            start = value_start + end + closing.len_utf8();
        }
    }
    for marker in ["感谢您使用", "感谢使用", "欢迎您使用", "欢迎使用"] {
        let mut start = 0;
        while let Some(found) = text[start..].find(marker) {
            let value_start = start + found + marker.len();
            let rest = text[value_start..].trim_start();
            let skipped = text[value_start..].len() - rest.len();
            let (value, next) = if let Some(body) = rest.strip_prefix('“') {
                match body.find('”') {
                    Some(end) => (
                        &body[..end],
                        value_start + skipped + '“'.len_utf8() + end + '”'.len_utf8(),
                    ),
                    None => break,
                }
            } else if let Some(body) = rest.strip_prefix('"') {
                match body.find('"') {
                    Some(end) => (&body[..end], value_start + skipped + 1 + end + 1),
                    None => break,
                }
            } else {
                let end = rest
                    .find(['，', ',', '。', '.', '；', ';', ':', '：'])
                    .unwrap_or(rest.len());
                (&rest[..end], value_start + skipped + end)
            };
            add_evidence(
                evidence,
                &value.replace("小程序", ""),
                "javascript:privacy-or-agreement",
                EvidenceFamily::Agreement,
                EvidenceTier::Primary,
                94,
            );
            start = next.max(value_start + 1);
            if start >= text.len() {
                break;
            }
        }
    }
    for (marker, source, weight) in [
        ("appName:", "javascript:appName", 96),
        ("mp_name:", "javascript:mp_name", 98),
        ("brandName:", "javascript:brandName", 90),
        ("productName:", "javascript:productName", 90),
    ] {
        let mut start = 0;
        while let Some((value, next)) = quoted_value_after(text, marker, start) {
            add_evidence(
                evidence,
                value,
                source,
                EvidenceFamily::IdentityField,
                EvidenceTier::Primary,
                weight,
            );
            start = next;
        }
    }
    let mut start = 0;
    while let Some((value, next)) = quoted_value_after(text, r#"navigationBarTitleText":"#, start) {
        add_evidence(
            evidence,
            value,
            "javascript:navigation-title",
            EvidenceFamily::Page,
            EvidenceTier::Fallback,
            25,
        );
        start = next;
    }
    for field in ["brandName", "appName", "mp_name", "productName"] {
        let mut start = 0;
        while let Some(found) = text[start..].find(field) {
            let field_end = start + found + field.len();
            let mut tail_end = (field_end + 240).min(text.len());
            while !text.is_char_boundary(tail_end) {
                tail_end -= 1;
            }
            let tail = &text[field_end..tail_end];
            if let Some((value, _)) = quoted_value_after(tail, "||", 0) {
                add_evidence(
                    evidence,
                    value,
                    "javascript:fallback-brand-name",
                    EvidenceFamily::IdentityField,
                    EvidenceTier::Context,
                    74,
                );
            }
            start = field_end;
        }
    }
    let mut start = 0;
    while let Some(found) = text[start..].find("pageTitle") {
        let section_start = start + found + "pageTitle".len();
        let mut section_end = (section_start + 1_000).min(text.len());
        while !text.is_char_boundary(section_end) {
            section_end -= 1;
        }
        if let Some((value, _)) = quoted_value_after(&text[section_start..section_end], "main:", 0)
        {
            add_evidence(
                evidence,
                value,
                "javascript:page-title-main",
                EvidenceFamily::PageMap,
                EvidenceTier::Fallback,
                54,
            );
        }
        start = section_start;
    }
    let mut start = 0;
    while let Some(found) = text[start..].find("pageTitle.") {
        let key_start = start + found + "pageTitle.".len();
        let key: String = text[key_start..]
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
            .collect();
        if !key.is_empty() {
            let marker = format!("{key}:");
            if let Some((value, _)) = quoted_value_after(text, &marker, 0) {
                add_evidence(
                    evidence,
                    value,
                    "javascript:page-title-reference",
                    EvidenceFamily::PageMap,
                    EvidenceTier::Context,
                    62,
                );
            }
        }
        start = key_start.saturating_add(key.len());
    }
    for marker in ["onShareAppMessage", "onShareTimeline"] {
        let mut start = 0;
        while let Some(found) = text[start..].find(marker) {
            let section_start = start + found + marker.len();
            let mut section_end = (section_start + 1_000).min(text.len());
            while !text.is_char_boundary(section_end) {
                section_end -= 1;
            }
            if let Some((value, _)) =
                quoted_value_after(&text[section_start..section_end], "title:", 0)
            {
                add_evidence(
                    evidence,
                    value,
                    "javascript:share-title",
                    EvidenceFamily::Share,
                    EvidenceTier::Fallback,
                    60,
                );
            }
            start = section_start;
        }
    }
}

fn rank(evidence: &[NameEvidence], primary_only: bool) -> Vec<RankedCandidate> {
    let mut grouped: HashMap<&str, Vec<&NameEvidence>> = HashMap::new();
    for item in evidence {
        if !primary_only || item.tier == EvidenceTier::Primary {
            grouped.entry(&item.value).or_default().push(item);
        }
    }
    let mut ranked = Vec::with_capacity(grouped.len());
    for (value, items) in grouped {
        let mut by_family: HashMap<EvidenceFamily, &NameEvidence> = HashMap::new();
        for &item in &items {
            by_family
                .entry(item.family)
                .and_modify(|current| {
                    if item.weight > current.weight {
                        *current = item;
                    }
                })
                .or_insert(item);
        }
        let mut independent: Vec<_> = by_family.into_values().collect();
        independent.sort_by_key(|item| std::cmp::Reverse(item.weight));
        let strongest = independent[0];
        let mut score = strongest.weight + name_quality_adjustment(value);
        for supporting in independent.iter().skip(1) {
            score += (supporting.weight / 4).clamp(4, 20) + 6;
        }
        let mut families: Vec<_> = independent.iter().map(|item| item.family).collect();
        #[cfg(test)]
        let mut sources: Vec<_> = independent.iter().map(|item| item.source).collect();
        for supporting in evidence {
            if supporting.value != value
                && related_variant(value, &supporting.value)
                && !families.contains(&supporting.family)
            {
                score += (supporting.weight / 4).clamp(4, 20) + 6;
                families.push(supporting.family);
                #[cfg(test)]
                sources.push(supporting.source);
            }
        }
        ranked.push(RankedCandidate {
            value: value.to_owned(),
            source: strongest.source,
            score,
            tier: strongest.tier,
            #[cfg(test)]
            family_count: families.len(),
            #[cfg(test)]
            sources,
            occurrences: items.len(),
        });
    }
    ranked.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| right.tier.cmp(&left.tier))
            .then_with(|| right.occurrences.cmp(&left.occurrences))
            .then_with(|| right.value.cmp(&left.value))
    });
    ranked
}

fn name_quality_adjustment(value: &str) -> i32 {
    let length = value.chars().count();
    let ascii = value.chars().filter(char::is_ascii_alphanumeric).count();
    let cjk = value
        .chars()
        .filter(|ch| ('\u{4e00}'..='\u{9fff}').contains(ch))
        .count();
    let mut adjustment = length.saturating_sub(3).min(6) as i32;
    if cjk == 0 && ascii > 0 {
        adjustment -= 16;
    } else if ascii >= 3 && cjk > 0 {
        let ascii_prefix: String = value.chars().take_while(char::is_ascii_lowercase).collect();
        adjustment -= if ascii_prefix.len() >= 3 { 24 } else { 4 };
    }
    if value.contains(['&', '|']) {
        adjustment -= 8;
    }
    if ["注册", "常用", "接收", "补充", "查看", "重启"]
        .iter()
        .any(|prefix| value.starts_with(prefix))
        || ["信息", "设置", "结果", "详情", "记录", "成功"]
            .iter()
            .any(|suffix| value.ends_with(suffix))
    {
        adjustment -= 12;
    }
    adjustment
}

fn related_variant(core: &str, extended: &str) -> bool {
    if core.chars().count() < 3 || !extended.starts_with(core) {
        let common_prefix = core
            .chars()
            .zip(extended.chars())
            .take_while(|(left, right)| left == right)
            .count();
        return common_prefix >= 4 && core.chars().count() == extended.chars().count();
    }
    let suffix = &extended[core.len()..];
    suffix.starts_with("官方")
        || suffix.starts_with('|')
        || suffix.starts_with('·')
        || suffix.starts_with('-')
}

fn decisive(ranked: &[RankedCandidate], minimum: i32, minimum_lead: i32) -> bool {
    let Some(best) = ranked.first() else {
        return false;
    };
    best.score >= minimum
        && ranked
            .get(1)
            .is_none_or(|runner_up| best.score - runner_up.score >= minimum_lead)
}

fn build_recognition(
    stage: &str,
    winner: &RankedCandidate,
    ranked: &[RankedCandidate],
) -> Recognition {
    Recognition {
        name: winner.value.clone(),
        source: format!("{stage}:{}", winner.source),
        confidence: winner.score.clamp(0, 100) as u8,
        candidates: ranked
            .iter()
            .take(8)
            .map(|candidate| candidate.value.clone())
            .collect(),
        #[cfg(test)]
        diagnostics: ranked
            .iter()
            .take(12)
            .map(|candidate| {
                format!(
                    "{} score={} tier={:?} families={} occurrences={} sources={}",
                    candidate.value,
                    candidate.score,
                    candidate.tier,
                    candidate.family_count,
                    candidate.occurrences,
                    candidate.sources.join("|")
                )
            })
            .collect(),
    }
}

fn collect_evidence(path: &std::path::Path) -> Option<Vec<NameEvidence>> {
    let archive = Archive::open(path).ok()?;
    let mut evidence = Vec::new();
    add_config_evidence(&archive, &mut evidence);
    for (name, bytes) in archive.files() {
        if name.ends_with(".js") {
            add_javascript_evidence(&String::from_utf8_lossy(bytes), &mut evidence);
        }
    }
    Some(evidence)
}

#[cfg(test)]
pub fn recognition_diagnostics(path: &std::path::Path) -> Vec<String> {
    collect_evidence(path)
        .map(|evidence| rank(&evidence, false))
        .unwrap_or_default()
        .into_iter()
        .take(20)
        .map(|candidate| {
            format!(
                "{} score={} tier={:?} families={} occurrences={} sources={}",
                candidate.value,
                candidate.score,
                candidate.tier,
                candidate.family_count,
                candidate.occurrences,
                candidate.sources.join("|")
            )
        })
        .collect()
}

/// Extracts evidence solely from a mini-program's own __APP__.wxapkg.
pub fn recognize_main_package(path: &std::path::Path) -> Option<Recognition> {
    let evidence = collect_evidence(path)?;

    let primary = rank(&evidence, true);
    if decisive(&primary, 88, 18) {
        return Some(build_recognition("format", &primary[0], &primary));
    }

    let voted = rank(&evidence, false);
    let minimum = match voted.first()?.tier {
        EvidenceTier::Primary => 72,
        EvidenceTier::Context => 68,
        EvidenceTier::Fallback => 70,
    };
    if decisive(&voted, minimum, 8) {
        return Some(build_recognition("vote", &voted[0], &voted));
    }
    let frequency_tiebreak = voted[0].tier == EvidenceTier::Context
        && voted[0].score >= 68
        && voted[0].occurrences >= 6
        && voted.get(1).is_some_and(|next| {
            voted[0].score >= next.score
                && voted[0].score - next.score <= 2
                && voted[0].occurrences >= next.occurrences.saturating_mul(2)
        });
    if frequency_tiebreak {
        return Some(build_recognition("frequency-tiebreak", &voted[0], &voted));
    }
    let mut repeated_pages = rank(
        &evidence
            .iter()
            .filter(|item| item.family == EvidenceFamily::Page)
            .cloned()
            .collect::<Vec<_>>(),
        false,
    );
    repeated_pages.sort_by(|left, right| {
        right
            .occurrences
            .cmp(&left.occurrences)
            .then_with(|| right.score.cmp(&left.score))
    });
    let repeated_page = repeated_pages.first().is_some_and(|best| {
        best.occurrences >= 3
            && best.value.chars().count() >= 4
            && best
                .value
                .chars()
                .any(|ch| ('\u{4e00}'..='\u{9fff}').contains(&ch))
            && repeated_pages
                .get(1)
                .is_none_or(|next| best.occurrences > next.occurrences)
    });
    if repeated_page {
        return Some(build_recognition(
            "page-frequency",
            &repeated_pages[0],
            &repeated_pages,
        ));
    }
    let page_only = !evidence.is_empty()
        && evidence
            .iter()
            .all(|item| item.family == EvidenceFamily::Page)
        && voted.len() <= 6
        && voted[0].value.chars().count() >= 5
        && voted
            .get(1)
            .is_none_or(|next| voted[0].value.chars().count() >= next.value.chars().count() + 2);
    page_only.then(|| build_recognition("page-fallback", &voted[0], &voted))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wxapkg::fixture;
    use std::fs;

    fn recognize(files: &[(&str, &[u8])]) -> Option<Recognition> {
        let path = std::env::temp_dir().join(format!(
            "wxapkg-recognition-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::write(&path, fixture(files)).unwrap();
        let result = recognize_main_package(&path);
        let _ = fs::remove_file(path);
        result
    }

    #[test]
    fn structured_global_title_wins_before_page_fallbacks() {
        let result = recognize(&[(
            "app-config.json",
            r#"{"global":{"window":{"navigationBarTitleText":"盒马鲜生"}},"page":{"a":{"window":{"navigationBarTitleText":"商品详情"}}}}"#.as_bytes(),
        )])
        .unwrap();
        assert_eq!(result.name, "盒马鲜生");
        assert!(result.source.starts_with("format:"));
    }

    #[test]
    fn malformed_config_does_not_hide_agreement_evidence() {
        let result = recognize(&[
            ("app-config.json", b"not json"),
            ("app-service.js", "欢迎您使用“探鱼烤鱼”小程序".as_bytes()),
        ])
        .unwrap();
        assert_eq!(result.name, "探鱼烤鱼");
    }

    #[test]
    fn rejects_dynamic_app_name_and_uses_program_phrase() {
        let result = recognize(&[(
            "app-service.js",
            "appName:\"+Y(D.appName)+\";本活动由小程序“小萝卜报名”发布".as_bytes(),
        )])
        .unwrap();
        assert_eq!(result.name, "小萝卜报名");
    }

    #[test]
    fn rejects_technical_slug_and_uses_repeated_page_brand() {
        let result = recognize(&[(
            "app-service.js",
            concat!(
                "appName:\"mobile-xcx-vegetable\";",
                "navigationBarTitleText\":\"多多买菜\";",
                "navigationBarTitleText\":\"多多买菜\";",
                "navigationBarTitleText\":\"多多买菜\";",
                "navigationBarTitleText\":\"多多买菜\";",
                "navigationBarTitleText\":\"多多买菜\";",
                "navigationBarTitleText\":\"分享助手\";",
                "navigationBarTitleText\":\"分享助手\";",
                "navigationBarTitleText\":\"分享助手\";",
                "navigationBarTitleText\":\"分享助手\";"
            )
            .as_bytes(),
        )])
        .unwrap();
        assert_eq!(result.name, "多多买菜");
        assert!(result.source.starts_with("page-frequency:"));
    }

    #[test]
    fn decodes_brand_fallbacks_without_app_specific_rules() {
        let result = recognize(&[(
            "app-service.js",
            r#"brandName)||"\u5c0f\u4e8c\u76f4\u79df""#.as_bytes(),
        )])
        .unwrap();
        assert_eq!(result.name, "小二直租");
    }

    #[test]
    fn repeated_weak_evidence_does_not_outvote_primary_evidence() {
        let result = recognize(&[
            (
                "app-config.json",
                r#"{"global":{"window":{"navigationBarTitleText":"可靠名称"}}}"#.as_bytes(),
            ),
            (
                "app-service.js",
                "onShareAppMessage(){return{title:\"促销活动\"}};onShareAppMessage(){return{title:\"促销活动\"}}"
                    .as_bytes(),
            ),
        ])
        .unwrap();
        assert_eq!(result.name, "可靠名称");
    }

    #[test]
    fn independent_sources_resolve_conflicting_primary_evidence() {
        let result = recognize(&[
            (
                "app-config.json",
                r#"{"global":{"window":{"navigationBarTitleText":"门店首页"}}}"#.as_bytes(),
            ),
            (
                "app-service.js",
                "appName:\"品牌商城\";欢迎您使用“品牌商城”小程序".as_bytes(),
            ),
        ])
        .unwrap();
        assert_eq!(result.name, "品牌商城");
        assert!(result.source.starts_with("format:"));
    }

    #[test]
    fn close_conflicting_primary_evidence_is_left_unrecognized() {
        let result = recognize(&[
            (
                "app-config.json",
                r#"{"global":{"window":{"navigationBarTitleText":"甲方服务"}}}"#.as_bytes(),
            ),
            ("app-service.js", "appName:\"乙方服务\"".as_bytes()),
        ]);
        assert!(result.is_none());
    }

    #[test]
    fn a_share_title_alone_is_not_an_identity() {
        let result = recognize(&[(
            "app-service.js",
            "onShareAppMessage(){return{title:\"邀请好友领券\"}}".as_bytes(),
        )]);
        assert!(result.is_none());
    }

    #[test]
    fn utf8_window_boundaries_do_not_panic() {
        let mut script = "预".repeat(500);
        script.push_str("brandName)||\"边界名称\"");
        let result = recognize(&[("app-service.js", script.as_bytes())]).unwrap();
        assert_eq!(result.name, "边界名称");
    }
}
