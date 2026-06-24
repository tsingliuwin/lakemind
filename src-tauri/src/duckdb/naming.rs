//! Source table-name generation.
//!
//! Turns an arbitrary (often Chinese) file name into a meaningful ASCII SQL
//! identifier. Strategy (hybrid):
//!
//! - [`pure_name`]: deterministic pure function — CJK → pinyin (via the `pinyin`
//!   crate), ASCII alphanumerics kept, everything else collapsed to `_`. Used as
//!   the immediate fallback and whenever the LLM is unavailable.
//! - [`llm_name`] / [`choose_name`]: ask the configured LLM for a short
//!   snake_case English name; fall back to [`pure_name`] on failure/timeout.
//!
//! The `s_` prefix (SOURCE) is added by [`view_name`]; the bare slug is kept
//! for the `label` so the original file name can be shown elsewhere.

use pinyin::ToPinyin;

/// Soft cap on the generated slug length. Pinyin of long Chinese names can be
/// very long; we trim rather than produce unwieldy identifiers.
const MAX_LEN: usize = 60;

/// Deterministic ASCII slug from an arbitrary (possibly Chinese) label.
///
/// - ASCII `[A-Za-z0-9_]` is kept (letters lower-cased for a stable look).
/// - CJK characters are converted to plain (tone-less) pinyin, e.g. `销售` →
///   `xiaoshou`.
/// - Any other character collapses to a single `_` (consecutive ones merged).
/// - Leading digit gets a `_` prefix (identifiers can't start with a digit).
/// - Trimmed of surrounding `_`, capped to [`MAX_LEN`] chars.
/// - If the result is empty/degenerate, the original label is hashed so distinct
///   Chinese-only files don't all collide on `source`.
pub fn pure_name(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut prev_under = false;
    for c in raw.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            // Lower-case ASCII letters for a stable look. A leading digit is fine
            // here: `view_name` always prepends `s_`, so the final identifier
            // starts with a letter.
            out.push(if c.is_ascii_alphabetic() {
                c.to_ascii_lowercase()
            } else {
                c
            });
            prev_under = false;
        } else if let Some(p) = c.to_pinyin() {
            // CJK → plain pinyin (no tones, already lowercase ASCII).
            out.push_str(p.plain());
            prev_under = false;
        } else if !prev_under {
            out.push('_');
            prev_under = true;
        }
    }

    let mut s: String = out.trim_matches('_').to_string();
    if s.is_empty() {
        s = "source".to_string();
    }
    // Cap length on a char boundary.
    if s.chars().count() > MAX_LEN {
        s = s
            .chars()
            .take(MAX_LEN)
            .collect::<String>()
            .trim_end_matches('_')
            .to_string();
    }
    // Degenerate (all-symbols) result → disambiguate with a short hash of the
    // original name so two different Chinese-only files don't share `source`.
    if s.is_empty() || s == "source" {
        s = format!("source_{}", fnv1a_hex(raw));
    }
    s
}

/// Full SQL view name for a source label: `s_<pure_name>`.
pub fn view_name(raw: &str) -> String {
    format!("s_{}", pure_name(raw))
}

/// FNV-1a 32-bit hash of a string as lowercase hex (disambiguation suffix).
fn fnv1a_hex(s: &str) -> String {
    let mut hash: u32 = 0x811c9dc5;
    for &b in s.as_bytes() {
        hash ^= b as u32;
        hash = hash.wrapping_mul(0x01000193);
    }
    format!("{:x}", hash)
}

/// Ask the configured LLM for a short snake_case English identifier for `raw`.
/// Returns `None` when no provider is configured, the call fails, times out
/// (≈8s), or the response isn't a clean identifier.
pub async fn llm_name(raw: &str) -> Option<String> {
    use std::time::Duration;

    let model_id = crate::agent::first_enabled_model()?;
    let prompt = format!(
        "You generate a SQL table identifier from a data file name. \
Reply with ONLY a short lowercase snake_case English identifier \
(letters/digits/underscore only, must start with a letter, <= 30 chars, no quotes, no explanation). \
File name: {raw}"
    );
    let result = tokio::time::timeout(
        Duration::from_secs(8),
        crate::agent::complete_one_shot(&prompt, &model_id),
    )
    .await
    .ok()?;
    let text = result.ok()?;
    let cleaned = sanitize_identifier(text.trim());
    if cleaned.is_empty() || cleaned == "source" {
        None
    } else {
        Some(cleaned)
    }
}

/// LLM-generated name if available, otherwise the deterministic [`pure_name`].
/// Returns the full view name (`s_<slug>`) and the source: `"llm"` (LLM ok) or
/// `"fallback"` (pinyin). The source is persisted so re-syncs skip the LLM call
/// once a name has settled.
pub async fn choose_name(raw: &str) -> (String, &'static str) {
    match llm_name(raw).await {
        Some(slug) => (format!("s_{}", slug), "llm"),
        None => (view_name(raw), "fallback"),
    }
}

/// Normalize an LLM response into a valid bare identifier (no `s_` prefix):
/// keep `[a-z0-9_]`, lower-case ASCII letters, collapse the rest to `_`, trim,
/// cap length, ensure it starts with a letter.
fn sanitize_identifier(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut prev_under = false;
    for c in raw.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            out.push(if c.is_ascii_alphabetic() {
                c.to_ascii_lowercase()
            } else {
                c
            });
            prev_under = false;
        } else if !prev_under {
            out.push('_');
            prev_under = true;
        }
    }
    let mut s = out.trim_matches('_').to_string();
    // Must start with a letter (not a digit / underscore).
    if s.chars().next().map_or(true, |c| !c.is_ascii_alphabetic()) {
        return String::new();
    }
    if s.chars().count() > MAX_LEN {
        s = s
            .chars()
            .take(MAX_LEN)
            .collect::<String>()
            .trim_end_matches('_')
            .to_string();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_passes_through_lowercased() {
        assert_eq!(pure_name("Sales_Data"), "sales_data");
        assert_eq!(view_name("Sales_Data"), "s_sales_data");
    }

    #[test]
    fn cjk_becomes_pinyin() {
        let name = pure_name("销售数据");
        assert!(name.starts_with("xiao"));
        assert!(!name.contains('销'));
        assert!(name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
    }

    #[test]
    fn mixed_filename_preserves_ascii_and_pinyin() {
        // ASCII parts kept, Chinese → pinyin, separators normalized.
        let name = pure_name("yantujy_销售管理看板_20260623_2140");
        assert!(name.starts_with("yantujy_"));
        assert!(name.contains("20260623_2140"));
        assert!(name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
    }

    #[test]
    fn symbols_collapse_to_single_underscore() {
        assert_eq!(pure_name("a.b-c d"), "a_b_c_d");
    }

    #[test]
    fn degenerate_name_is_disambiguated() {
        // Pure-symbol input would collapse to "source"; the hash suffix keeps
        // two distinct symbol-only names from colliding.
        let a = pure_name("!!@@");
        let b = pure_name("##$$");
        assert!(a.starts_with("source_"));
        assert!(b.starts_with("source_"));
        assert_ne!(a, b);
    }

    #[test]
    fn leading_digit_is_valid_via_s_prefix() {
        // pure_name itself may start with a digit; view_name prepends `s_` so the
        // final SQL identifier always starts with a letter.
        assert_eq!(pure_name("123abc"), "123abc");
        assert_eq!(view_name("123abc"), "s_123abc");
    }
}
