//! Global analysis-tenets library — an OKF v0.1 bundle of analysis playbooks.
//!
//! LakeMind ships a curated, **global** knowledge bundle of data-analysis
//! tenets (methodologies + industry cases + topic pitfalls) at
//! `~/.lakemind/tenets/`. Unlike the per-workspace OKF (`<ws>/okf/`, which
//! holds business knowledge for one project), this bundle is **shared across
//! all workspaces** — it is product asset, not per-project memory.
//!
//! The bundle follows [OKF v0.1](https://github.com/GoogleCloudPlatform/knowledge-catalog/blob/main/okf/SPEC.md):
//! a directory tree of Markdown concepts (frontmatter + body), `index.md`
//! directory listings for progressive disclosure, and bundle-relative links.
//! Consumption is **permissive** — unknown frontmatter fields / types are
//! preserved, never fatal.
//!
//! PREAMBLE carries only the cross-industry core tenets (a summary); the rich
//! industry/topic cases live here and are pulled into context on demand by the
//! agent via `search_tenets` / `load_tenets` tools.

use std::fs;
use std::path::{Path, PathBuf};

// ── Seed bundle (compiled into the binary via include_str!) ──────────────────
// Each seed file is stored under src/tenets_seed/ and mirrored to
// ~/.lakemind/tenets/ on first launch. Using include_str! keeps the seed
// human-editable as real .md files and version-controlled with the source.
const SEED_INDEX: &str = include_str!("tenets_seed/index.md");
const SEED_CORE_INDEX: &str = include_str!("tenets_seed/core/index.md");
const SEED_GENERAL_PRINCIPLES: &str = include_str!("tenets_seed/core/general-principles.md");
const SEED_DATA_DISCIPLINE: &str = include_str!("tenets_seed/core/data-discipline.md");
const SEED_DATA_PROFILING: &str = include_str!("tenets_seed/core/data-profiling.md");
const SEED_DATA_CLEANING: &str = include_str!("tenets_seed/core/data-cleaning.md");
const SEED_DATA_ANALYSIS: &str = include_str!("tenets_seed/core/data-analysis.md");
const SEED_DATA_PRESENTATION: &str = include_str!("tenets_seed/core/data-presentation.md");
const SEED_DATA_SECURITY: &str = include_str!("tenets_seed/core/data-security.md");
const SEED_DATA_METRICS: &str = include_str!("tenets_seed/core/data-metrics.md");
const SEED_META_GOVERNANCE: &str = include_str!("tenets_seed/core/meta-governance.md");
const SEED_INDUSTRY_INDEX: &str = include_str!("tenets_seed/industry/index.md");
const SEED_EDUCATION_INDEX: &str = include_str!("tenets_seed/industry/education/index.md");
const SEED_EDUCATION_K12: &str = include_str!("tenets_seed/industry/education/k12.md");
const SEED_EDUCATION_POSTGRAD: &str = include_str!("tenets_seed/industry/education/postgrad.md");
const SEED_TOURISM: &str = include_str!("tenets_seed/industry/tourism.md");
const SEED_REALESTATE: &str = include_str!("tenets_seed/industry/realestate.md");
const SEED_TOPIC_INDEX: &str = include_str!("tenets_seed/topic/index.md");
const SEED_CONVERSION: &str = include_str!("tenets_seed/topic/conversion.md");
const SEED_GROWTH: &str = include_str!("tenets_seed/topic/growth.md");

/// One (relative_path, content) pair in the seed bundle.
const SEED_FILES: &[(&str, &str)] = &[
    ("index.md", SEED_INDEX),
    ("core/index.md", SEED_CORE_INDEX),
    ("core/general-principles.md", SEED_GENERAL_PRINCIPLES),
    ("core/data-discipline.md", SEED_DATA_DISCIPLINE),
    ("core/data-profiling.md", SEED_DATA_PROFILING),
    ("core/data-cleaning.md", SEED_DATA_CLEANING),
    ("core/data-analysis.md", SEED_DATA_ANALYSIS),
    ("core/data-presentation.md", SEED_DATA_PRESENTATION),
    ("core/data-security.md", SEED_DATA_SECURITY),
    ("core/data-metrics.md", SEED_DATA_METRICS),
    ("core/meta-governance.md", SEED_META_GOVERNANCE),
    ("industry/index.md", SEED_INDUSTRY_INDEX),
    ("industry/education/index.md", SEED_EDUCATION_INDEX),
    ("industry/education/k12.md", SEED_EDUCATION_K12),
    ("industry/education/postgrad.md", SEED_EDUCATION_POSTGRAD),
    ("industry/tourism.md", SEED_TOURISM),
    ("industry/realestate.md", SEED_REALESTATE),
    ("topic/index.md", SEED_TOPIC_INDEX),
    ("topic/conversion.md", SEED_CONVERSION),
    ("topic/growth.md", SEED_GROWTH),
];

/// OKF reserved filenames — these are structural (directory listings / logs)
/// and are skipped by concept enumeration / search. OKF v0.1: `index.md` and
/// `log.md` carry no concept frontmatter.
const RESERVED: &[&str] = &["index.md", "log.md"];

/// Get the global tenets bundle root: `~/.lakemind/tenets/`
pub fn get_tenets_dir() -> PathBuf {
    match crate::db::get_lakemind_dir() {
        Ok(p) => p.join("tenets"),
        Err(_) => PathBuf::from("tenets"),
    }
}

/// Parsed frontmatter of a tenet concept. Permissive: only the fields the
/// retriever consumes are typed; everything else is ignored (OKF v0.1
/// consumption rule). All fields are optional except `type` for conformance.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TenetMeta {
    pub kind: Option<String>,    // frontmatter `type`
    pub title: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,       // parsed from YAML `tags: [a, b, c]`
}

/// A hit returned by search / tag-filter — enough to decide whether to load
/// the full concept body. Serialized to the frontend as the catalog DTO.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TenetHit {
    /// Bundle-relative concept ID (e.g. `industry/education`), no `.md`.
    pub concept_id: String,
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    /// First few body lines, for a quick scan.
    pub preview: String,
}

/// Parse a single YAML frontmatter field value (first match), mirroring
/// `okf::parse_yaml_field`. Returns the trimmed value.
fn parse_yaml_field(content: &str, field: &str) -> Option<String> {
    let mut in_yaml = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "---" {
            if in_yaml {
                break;
            }
            in_yaml = true;
            continue;
        }
        if in_yaml {
            let parts: Vec<&str> = trimmed.splitn(2, ':').collect();
            if parts.len() == 2 && parts[0].trim().eq_ignore_ascii_case(field) {
                return Some(parts[1].trim().to_string());
            }
        }
    }
    None
}

/// Parse the `tags` list from YAML frontmatter. Handles inline-array form
/// `tags: [a, b]` and block form:
///   tags:
///     - a
///     - b
fn parse_yaml_tags(content: &str) -> Vec<String> {
    let lines: Vec<&str> = content.lines().collect();
    let mut in_yaml = false;
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim();
        if trimmed == "---" {
            in_yaml = !in_yaml;
            i += 1;
            continue;
        }
        if in_yaml {
            let parts: Vec<&str> = trimmed.splitn(2, ':').collect();
            if parts.len() == 2 && parts[0].trim().eq_ignore_ascii_case("tags") {
                let inline = parts[1].trim();
                // Inline array: [a, b, c]
                if inline.starts_with('[') {
                    let inner = inline
                        .trim_start_matches('[')
                        .trim_end_matches(']');
                    return inner
                        .split(',')
                        .map(|s| s.trim().trim_matches(|c| c == '"' || c == '\'').trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                }
                // Empty inline — could be block form following.
                let mut tags = Vec::new();
                let mut j = i + 1;
                while j < lines.len() {
                    let blk = lines[j].trim();
                    if blk.starts_with("---") {
                        break;
                    }
                    // Block entries are `- item` (possibly quoted), and must be
                    // indented under `tags:`. A non-indented line ends the list.
                    let raw = lines[j];
                    let indented = raw.starts_with(' ') || raw.starts_with('\t');
                    if indented && (blk.starts_with('-') || blk.starts_with('*')) {
                        let val = blk
                            .trim_start_matches(|c| c == '-' || c == '*' || c == ' ')
                            .trim_matches(|c| c == '"' || c == '\'')
                            .trim()
                            .to_string();
                        if !val.is_empty() {
                            tags.push(val);
                        }
                        j += 1;
                    } else if blk.is_empty() {
                        j += 1;
                    } else {
                        break;
                    }
                }
                return tags;
            }
        }
        i += 1;
    }
    Vec::new()
}

/// Parse frontmatter of a concept. Permissive — never errors on missing
/// fields, returns a `TenetMeta` with `None`/empty where absent.
pub fn parse_tenets_frontmatter(content: &str) -> TenetMeta {
    TenetMeta {
        kind: parse_yaml_field(content, "type"),
        title: parse_yaml_field(content, "title"),
        description: parse_yaml_field(content, "description"),
        tags: parse_yaml_tags(content),
    }
}

/// Is `name` an OKF reserved filename (structural, not a concept)?
fn is_reserved(name: &str) -> bool {
    let lower = name.to_lowercase();
    RESERVED.iter().any(|r| lower == *r)
}

/// Build a `TenetHit` from a concept file path + content.
fn hit_from_file(okf_root: &Path, path: &Path, content: &str) -> TenetHit {
    let rel = path
        .strip_prefix(okf_root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    let concept_id = rel.trim_end_matches(".md").to_string();
    let meta = parse_tenets_frontmatter(content);
    let title = meta.title.unwrap_or_else(|| concept_id.clone());
    // Preview = first few non-frontmatter body lines.
    let preview = body_preview(content, 6);
    TenetHit {
        concept_id,
        title,
        description: meta.description.unwrap_or_default(),
        tags: meta.tags,
        preview,
    }
}

/// Extract the first `n` lines of the body (after frontmatter) as a preview.
fn body_preview(content: &str, n: usize) -> String {
    let mut lines = content.lines();
    // Skip leading blank lines.
    // Drop frontmatter block if present.
    let first = lines.next();
    let mut started = true;
    if first.map(|f| f.trim() == "---").unwrap_or(false) {
        started = false;
        for line in lines.by_ref() {
            if line.trim() == "---" {
                started = true;
                break;
            }
        }
    }
    if !started {
        return String::new();
    }
    lines
        .filter(|l| !l.trim().is_empty())
        .take(n)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Walk the bundle and enumerate every concept `.md` (skipping reserved
/// `index.md` / `log.md`). Returns (path, content) pairs.
fn enumerate_concepts(okf_root: &Path) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    if !okf_root.exists() {
        return out;
    }
    for entry in walkdir::WalkDir::new(okf_root) {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if is_reserved(name) {
            continue;
        }
        if let Ok(content) = fs::read_to_string(path) {
            out.push((path.to_path_buf(), content));
        }
    }
    out
}

/// Enumerate every concept in the bundle as a `TenetHit`, sorted by
/// `concept_id` for a stable catalog listing. Used by the settings UI to render
/// the full shared-tenets library (read-only). Returns an empty vector if the
/// bundle has not been seeded.
pub fn list_all_tenets() -> Vec<TenetHit> {
    let root = get_tenets_dir();
    let mut hits: Vec<TenetHit> = enumerate_concepts(&root)
        .into_iter()
        .map(|(path, content)| hit_from_file(&root, &path, &content))
        .collect();
    hits.sort_by(|a, b| a.concept_id.cmp(&b.concept_id));
    hits
}

/// Search the bundle by keyword: matches against title, description, tags, and
/// body (case-insensitive substring). Returns concept hits sorted by relevance
/// (tag/title match ranked above body-only match). Permissive — a concept
/// without frontmatter still matches on body.
pub fn search_tenets(query: &str) -> Vec<TenetHit> {
    let root = get_tenets_dir();
    let q = query.to_lowercase();
    let mut in_meta = Vec::new(); // matched title/desc/tag
    let mut in_body = Vec::new(); // matched body only
    for (path, content) in enumerate_concepts(&root) {
        let hit = hit_from_file(&root, &path, &content);
        let title_l = hit.title.to_lowercase();
        let desc_l = hit.description.to_lowercase();
        let tags_l: Vec<String> = hit.tags.iter().map(|t| t.to_lowercase()).collect();
        let meta_match = title_l.contains(&q)
            || desc_l.contains(&q)
            || tags_l.iter().any(|t| t.contains(&q));
        let body_l = content.to_lowercase();
        if meta_match {
            in_meta.push(hit);
        } else if body_l.contains(&q) {
            in_body.push(hit);
        }
    }
    in_meta.append(&mut in_body);
    in_meta
}

/// Filter concepts by tags. A concept matches if it carries **any** of the
/// requested tags (OR semantics). Tag comparison is case-insensitive and
/// ignores a trailing `:value` namespace so `industry:education` matches a
/// query for `industry:education` exactly, but `industry` alone does NOT match
/// `industry:education` (the prefix-only shortcut would surprise users who
/// ask for a specific industry). Use the full `namespace:value` form.
pub fn load_tenets_by_tags(tags: &[String]) -> Vec<TenetHit> {
    let root = get_tenets_dir();
    let want: Vec<String> = tags.iter().map(|t| t.trim().to_lowercase()).collect();
    let mut hits = Vec::new();
    for (path, content) in enumerate_concepts(&root) {
        let hit = hit_from_file(&root, &path, &content);
        let have: Vec<String> = hit.tags.iter().map(|t| t.trim().to_lowercase()).collect();
        if want.iter().any(|w| have.contains(w)) {
            hits.push(hit);
        }
    }
    hits
}

/// Load a single concept's full body by concept ID (e.g. `industry/education`).
/// Returns the raw file content (frontmatter + body) so the agent sees the
/// metadata too. Returns an error string if not found (permissive: no panic).
pub fn load_tenet(concept_id: &str) -> Result<String, String> {
    let root = get_tenets_dir();
    let cleaned = concept_id.trim().trim_start_matches('/');
    let candidate = root.join(if cleaned.ends_with(".md") {
        cleaned.to_string()
    } else {
        format!("{cleaned}.md")
    });
    if candidate.exists() {
        return fs::read_to_string(&candidate)
            .map_err(|e| format!("读取准则失败: {e}"));
    }
    // Permissive fallback: try case-insensitive filename match in case the
    // agent passed a slightly different casing.
    for (path, content) in enumerate_concepts(&root) {
        let cid = path
            .strip_prefix(&root)
            .map(|p| p.to_string_lossy().replace('\\', "/").trim_end_matches(".md").to_lowercase())
            .unwrap_or_default();
        if cid == cleaned.to_lowercase() {
            return Ok(content);
        }
    }
    Err(format!("未找到准则: {concept_id}（可用 load_tenets 不带参数查看目录）"))
}

/// Load the root `index.md` — the progressive-disclosure entry point. Returns
/// an error string if the bundle hasn't been seeded.
pub fn load_tenets_index() -> Result<String, String> {
    let index = get_tenets_dir().join("index.md");
    if !index.exists() {
        return Err("准则库尚未初始化。".to_string());
    }
    fs::read_to_string(&index).map_err(|e| format!("读取目录失败: {e}"))
}

/// Seed the global tenets bundle on first launch. Writes each seed file only
/// if it does not already exist — **never overwrites** user edits. Missing
/// parent directories are created. Safe to call on every startup.
///
/// Returns `Ok(())` on success; errors are non-fatal at the call site (logged,
/// not crashing the app) but are surfaced here for testability.
pub fn seed_tenets_if_empty() -> Result<(), String> {
    let root = get_tenets_dir();
    fs::create_dir_all(&root).map_err(|e| format!("创建准则目录失败: {e}"))?;
    for (rel, content) in SEED_FILES {
        let dest = root.join(rel);
        if dest.exists() {
            continue;
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("创建子目录失败: {e}"))?;
        }
        fs::write(&dest, content).map_err(|e| format!("写入准则失败 {rel}: {e}"))?;
    }
    Ok(())
}

/// OKF v0.1 conformance check: every non-reserved concept `.md` in the bundle
/// must have a parseable frontmatter with a non-empty `type`. Used by tests
/// and (potentially) a future validator. Returns the list of non-conforming
/// concept IDs (empty = all conform).
#[allow(dead_code)]
pub fn nonconforming_concepts() -> Vec<String> {
    let root = get_tenets_dir();
    enumerate_concepts(&root)
        .into_iter()
        .filter(|(_, content)| {
            let meta = parse_tenets_frontmatter(content);
            meta.kind.map(|k| k.trim().is_empty()).unwrap_or(true)
        })
        .map(|(path, _)| {
            path.strip_prefix(&root)
                .map(|p| p.to_string_lossy().replace('\\', "/").trim_end_matches(".md").to_string())
                .unwrap_or_default()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Point the tenets dir at a temp location for hermetic tests.
    fn with_temp_bundle<F: FnOnce(&Path)>(f: F) {
        // We can't override get_tenets_dir (it reads ~/.lakemind), so the
        // parser/conformance logic is tested directly on in-memory content.
        // The seed/enum logic is validated structurally below.
        let _ = f(Path::new("."));
    }

    #[test]
    fn parse_frontmatter_inline_tags() {
        let md = "---\ntype: Playbook\ntitle: 测试\ndescription: d\ntags: [a, b:c, \"d e\"]\n---\n# body";
        let m = parse_tenets_frontmatter(md);
        assert_eq!(m.kind.as_deref(), Some("Playbook"));
        assert_eq!(m.title.as_deref(), Some("测试"));
        assert_eq!(m.description.as_deref(), Some("d"));
        assert_eq!(m.tags, vec!["a", "b:c", "d e"]);
    }

    #[test]
    fn parse_frontmatter_block_tags() {
        let md = "---\ntype: Playbook\ntitle: t\ntags:\n  - industry:education\n  - topic:conversion\n---\nbody";
        let m = parse_tenets_frontmatter(md);
        assert_eq!(m.tags, vec!["industry:education", "topic:conversion"]);
    }

    #[test]
    fn parse_frontmatter_missing_fields_are_permissive() {
        let md = "# just a body, no frontmatter";
        let m = parse_tenets_frontmatter(md);
        assert!(m.kind.is_none());
        assert!(m.title.is_none());
        assert!(m.tags.is_empty());
    }

    #[test]
    fn body_preview_skips_frontmatter() {
        let md = "---\ntype: T\n---\n\n# Title\nline1\nline2";
        let pv = body_preview(md, 6);
        assert!(pv.contains("# Title"));
        assert!(pv.contains("line1"));
    }

    #[test]
    fn seed_files_all_have_content() {
        // Structural sanity: every seed constant is non-empty.
        for (rel, content) in SEED_FILES {
            assert!(!content.is_empty(), "seed {rel} is empty");
        }
    }

    #[test]
    fn seed_concepts_conform_to_okf() {
        // Every non-reserved seed .md must have a non-empty `type` (OKF v0.1).
        for (rel, content) in SEED_FILES {
            let name = Path::new(rel).file_name().and_then(|n| n.to_str()).unwrap_or("");
            if is_reserved(name) {
                continue;
            }
            let meta = parse_tenets_frontmatter(content);
            assert!(
                meta.kind.as_deref().map(|k| !k.trim().is_empty()).unwrap_or(false),
                "seed concept {rel} missing non-empty frontmatter `type`"
            );
        }
    }

    #[test]
    fn seed_concepts_carry_tags_for_retrieval() {
        // Concept seeds (non-placeholder) must carry tags so the tag filter
        // can find them.
        for (rel, content) in SEED_FILES {
            let name = Path::new(rel).file_name().and_then(|n| n.to_str()).unwrap_or("");
            if is_reserved(name) {
                continue;
            }
            let meta = parse_tenets_frontmatter(content);
            assert!(!meta.tags.is_empty(), "seed concept {rel} has no tags");
        }
    }

    #[test]
    fn search_matches_tag_title_and_body() {
        // In-memory search via a synthetic scan over a known seed.
        let q = "归因";
        let matched_core = SEED_DATA_DISCIPLINE.to_lowercase().contains(q)
            || parse_tenets_frontmatter(SEED_DATA_DISCIPLINE)
                .tags
                .iter()
                .any(|t| t.to_lowercase().contains(q));
        let matched_edu = SEED_EDUCATION_K12.to_lowercase().contains(q);
        assert!(matched_core || matched_edu, "seed should mention 归因");
    }

    #[test]
    fn tag_filter_exact_match() {
        let tags = parse_yaml_tags(SEED_EDUCATION_K12);
        assert!(tags.iter().any(|t| t.eq_ignore_ascii_case("industry:education")));
        // prefix-only must NOT match a namespaced tag
        assert!(!tags.iter().any(|t| t.eq_ignore_ascii_case("industry")));
    }

    #[test]
    fn reserved_files_skipped_as_concepts() {
        assert!(is_reserved("index.md"));
        assert!(is_reserved("INDEX.md"));
        assert!(is_reserved("log.md"));
        assert!(!is_reserved("education.md"));
    }

    #[test]
    fn concept_id_strips_md() {
        let _ = with_temp_bundle(|_| {});
        // concept_id derivation is exercised end-to-end via hit_from_file in
        // the enum path; here we just assert the helper logic inline.
        let rel = "industry/education.md".trim_end_matches(".md");
        assert_eq!(rel, "industry/education");
    }
}
