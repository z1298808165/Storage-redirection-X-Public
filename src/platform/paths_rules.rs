// 排除规则与通配符路径的匹配。
//
// 规则匹配是配置校验、挂载回退和只读判定的公共前提；同一条规则在不同入口必须
// 得到一致结果，因此匹配实现只保留这一份。

use crate::platform::paths::{
    eq_ignore_case, is_child, normalize, sort_dedup_paths_case_insensitive, starts_with_ignore_case,
};

// is_recursive 允许 target 深入 rule 之下任意层级
pub fn matches(rule_path: &str, target_path: &str, is_recursive: bool) -> bool {
    if rule_path.is_empty() || target_path.is_empty() {
        return false;
    }

    if !contains_wildcards(rule_path) && !rule_path.contains("//") && !target_path.contains("//") {
        let rule = trim_match_slashes(rule_path);
        let target = trim_match_slashes(target_path);
        if rule.is_empty() || target.is_empty() {
            return false;
        }
        if target.eq_ignore_ascii_case(rule) {
            return true;
        }
        return is_recursive
            && target.len() > rule.len()
            && target.as_bytes().get(rule.len()) == Some(&b'/')
            && starts_with_ignore_case(target, rule);
    }

    let rule_segments: Vec<&str> = rule_path.split('/').filter(|s| !s.is_empty()).collect();
    let target_segments: Vec<&str> = target_path.split('/').filter(|s| !s.is_empty()).collect();
    if rule_segments.is_empty() || target_segments.len() < rule_segments.len() {
        return false;
    }

    for (rule_segment, target_segment) in rule_segments.iter().zip(target_segments.iter()) {
        if !match_segment_pattern(rule_segment, target_segment) {
            return false;
        }
    }

    if target_segments.len() == rule_segments.len() {
        return true;
    }

    is_recursive
}

pub fn contains_wildcards(path: &str) -> bool {
    path.contains('*') || path.contains('?')
}

pub fn split_exclusion_rules(rules: &[String]) -> (Vec<String>, Vec<String>) {
    let mut includes = Vec::new();
    let mut excludes = Vec::new();
    for rule in rules {
        let trimmed = rule.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(stripped) = trimmed.strip_prefix('!') {
            let stripped = stripped.trim_start();
            if !stripped.is_empty() {
                excludes.push(stripped.to_string());
            }
        } else {
            includes.push(trimmed.to_string());
        }
    }
    sort_dedup_paths_case_insensitive(&mut includes);
    sort_dedup_paths_case_insensitive(&mut excludes);
    (includes, excludes)
}

pub fn overlapping_exclusion_rules(includes: &[String], excludes: &[String]) -> Vec<String> {
    let mut effective: Vec<String> = excludes
        .iter()
        .filter(|excluded| {
            includes.iter().any(|included| {
                matches(included, excluded, true) || matches(excluded, included, true)
            })
        })
        .cloned()
        .collect();
    sort_dedup_paths_case_insensitive(&mut effective);
    effective
}

pub fn concrete_prefix_before_wildcard(path: &str) -> String {
    let normalized = normalize(path);
    if normalized.is_empty() || !contains_wildcards(&normalized) {
        return normalized;
    }

    let mut kept = Vec::new();
    for segment in normalized.split('/').filter(|segment| !segment.is_empty()) {
        if contains_wildcards(segment) {
            break;
        }
        kept.push(segment);
    }
    if kept.is_empty() {
        return String::new();
    }

    let prefix = kept.join("/");
    if normalized.starts_with('/') {
        normalize(&format!("/{prefix}"))
    } else {
        normalize(&prefix)
    }
}

pub fn wildcard_mount_fallback_parent(resolved_path: &str, storage_root: &str) -> Option<String> {
    let normalized = normalize(resolved_path);
    if normalized.is_empty() || !contains_wildcards(&normalized) {
        return None;
    }

    let prefix = concrete_prefix_before_wildcard(&normalized);
    if prefix.is_empty()
        || eq_ignore_case(&prefix, storage_root)
        || !is_child(&prefix, storage_root)
    {
        return None;
    }

    Some(prefix)
}

pub fn wildcard_policy_fallback_parent(resolved_path: &str, storage_root: &str) -> Option<String> {
    let normalized = normalize(resolved_path);
    if is_terminal_file_wildcard_rule(&normalized) {
        return None;
    }
    wildcard_mount_fallback_parent(&normalized, storage_root)
}

fn is_terminal_file_wildcard_rule(path: &str) -> bool {
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    let Some(last) = segments.next_back() else {
        return false;
    };
    contains_wildcards(last) && last.contains('.')
}

pub fn matches_xldownload_alias(rule_path: &str, target_path: &str) -> bool {
    if rule_path.is_empty() || target_path.is_empty() {
        return false;
    }

    let rule_segments: Vec<&str> = rule_path
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    let target_segments: Vec<&str> = target_path
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    if rule_segments.is_empty() || target_segments.len() < rule_segments.len() {
        return false;
    }

    for (rule_segment, target_segment) in rule_segments.iter().zip(target_segments.iter()) {
        if rule_segment.eq_ignore_ascii_case(".xldownload")
            && target_segment.eq_ignore_ascii_case(".xldownload")
        {
            continue;
        }
        if rule_segment != target_segment {
            return false;
        }
    }
    true
}

fn trim_match_slashes(path: &str) -> &str {
    path.trim_end_matches('/')
}

// 支持 * 和 ? 通配，单段匹配不跨 /
fn match_segment_pattern(pattern: &str, text: &str) -> bool {
    if !pattern.contains('*') && !pattern.contains('?') {
        return pattern.eq_ignore_ascii_case(text);
    }

    let pattern_chars: Vec<char> = pattern.chars().collect();
    let text_chars: Vec<char> = text.chars().collect();
    let mut pattern_idx = 0usize;
    let mut text_idx = 0usize;
    let mut star_idx: Option<usize> = None;
    let mut match_idx = 0usize;

    while text_idx < text_chars.len() {
        if pattern_idx < pattern_chars.len()
            && (pattern_chars[pattern_idx] == '?'
                || pattern_chars[pattern_idx].eq_ignore_ascii_case(&text_chars[text_idx]))
        {
            pattern_idx += 1;
            text_idx += 1;
            continue;
        }

        if pattern_idx < pattern_chars.len() && pattern_chars[pattern_idx] == '*' {
            star_idx = Some(pattern_idx);
            match_idx = text_idx;
            pattern_idx += 1;
            continue;
        }

        if let Some(star_pos) = star_idx {
            pattern_idx = star_pos + 1;
            match_idx += 1;
            text_idx = match_idx;
            continue;
        }

        return false;
    }

    while pattern_idx < pattern_chars.len() && pattern_chars[pattern_idx] == '*' {
        pattern_idx += 1;
    }

    pattern_idx == pattern_chars.len()
}
