// SPDX-License-Identifier: GPL-3.0-or-later
//! Deterministic question intent and measured-target binding for Ask.
//!
//! UI selection is context. Explicit app/folder names take precedence, and
//! broad cleanup questions stay broad. An unresolved name never grants access
//! to unrelated cleanup targets.

use crate::{
    agent_tools::{self, ToolWorld},
    ai,
    care::Target,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
};

const MAX_CANDIDATES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Intent {
    GlobalCleanup,
    TargetCleanup,
    Inspect,
    Growth,
    Capacity,
    Processes,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionScope {
    pub intent: Intent,
    pub target: Option<PathBuf>,
    pub label: String,
    pub clarification: Option<String>,
}

impl QuestionScope {
    pub fn is_targeted(&self) -> bool {
        self.target.is_some() || self.intent == Intent::TargetCleanup
    }

    /// A target owns only its exact subtree, including equivalent APFS paths.
    pub fn allows_path(&self, path: &Path) -> bool {
        if self.intent == Intent::Unsupported {
            return false;
        }
        match &self.target {
            Some(target) => normalize(path).starts_with(normalize(target)),
            None => self.intent != Intent::TargetCleanup && self.clarification.is_none(),
        }
    }
}

fn normalize(path: &Path) -> PathBuf {
    let path = path
        .strip_prefix("/System/Volumes/Data")
        .map(|suffix| Path::new("/").join(suffix))
        .unwrap_or_else(|_| path.to_path_buf());
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn has(words: &[String], choices: &[&str]) -> bool {
    words.iter().any(|word| choices.contains(&word.as_str()))
}

fn phrase_matches(text: &str, phrase: &str) -> bool {
    !phrase.is_empty()
        && text.match_indices(phrase).any(|(start, _)| {
            let end = start + phrase.len();
            text[..start]
                .chars()
                .next_back()
                .is_none_or(|c| !c.is_alphanumeric())
                && text[end..]
                    .chars()
                    .next()
                    .is_none_or(|c| !c.is_alphanumeric())
        })
}

fn generic(name: &str) -> bool {
    matches!(
        name,
        "a" | "an"
            | "the"
            | "my"
            | "this"
            | "that"
            | "these"
            | "those"
            | "please"
            | "can"
            | "could"
            | "you"
            | "i"
            | "how"
            | "what"
            | "which"
            | "is"
            | "are"
            | "it"
            | "to"
            | "of"
            | "for"
            | "from"
            | "in"
            | "on"
            | "and"
            | "or"
            | "with"
            | "all"
            | "any"
            | "some"
            | "up"
            | "out"
            | "safely"
            | "safe"
            | "quickly"
            | "more"
            | "extra"
            | "enough"
            | "scan"
            | "add"
            | "adds"
            | "used"
            | "using"
            | "usage"
            | "uses"
            | "does"
            | "doesn"
            | "t"
            | "get"
            | "getting"
            | "release"
            | "free"
            | "freeing"
            | "reclaim"
            | "recover"
            | "need"
            | "want"
            | "was"
            | "were"
            | "over"
            | "time"
            | "recently"
            | "last"
            | "week"
            | "day"
            | "month"
            | "today"
            | "yesterday"
            | "grew"
            | "growth"
            | "changed"
            | "change"
            | "increased"
            | "bigger"
            | "unnecessary"
            | "unneeded"
            | "temporary"
            | "rebuildable"
            | "old"
            | "older"
            | "legacy"
            | "leftover"
            | "leftovers"
            | "unused"
            | "cache"
            | "caches"
            | "files"
            | "file"
            | "data"
            | "folder"
            | "folders"
            | "directory"
            | "directories"
            | "app"
            | "apps"
            | "application"
            | "applications"
            | "space"
            | "storage"
            | "disk"
            | "system"
            | "mac"
            | "computer"
            | "drive"
            | "library"
            | "users"
            | "user"
            | "application support"
            | "containers"
            | "group containers"
            | "gb"
            | "gib"
            | "mb"
            | "mib"
            | "tb"
            | "tib"
    ) || name.chars().next().is_some_and(char::is_numeric)
}

#[derive(Debug)]
struct Candidate {
    path: PathBuf,
    aliases: Vec<String>,
}

fn aliases(path: &Path, label: Option<&str>) -> Vec<String> {
    let mut names = Vec::new();
    if let Some(label) = label {
        let label = label.to_lowercase();
        names.push(label.clone());
        for suffix in [" user data", " cache", " caches", " data"] {
            if let Some(name) = label.strip_suffix(suffix) {
                names.push(name.to_string());
            }
        }
    }
    if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
        names.push(name.to_lowercase());
    }
    // A rule may expose Cursor/User or an app container rather than the app's
    // measured root. Its owner name still binds only that known rule/path.
    let components: Vec<_> = path
        .components()
        .filter_map(|part| part.as_os_str().to_str())
        .collect();
    for pair in components.windows(2) {
        if matches!(
            pair[0],
            "Application Support" | "Caches" | "Containers" | "Group Containers"
        ) {
            names.push(pair[1].to_lowercase());
        }
    }
    let additional: Vec<_> = names
        .iter()
        .filter_map(|name| {
            if let Some(name) = name.strip_suffix(".app") {
                Some(name.to_string())
            } else if name.starts_with("com.")
                || name.starts_with("org.")
                || name.starts_with("net.")
            {
                name.rsplit('.').next().map(str::to_string)
            } else {
                None
            }
        })
        .collect();
    names.extend(additional);
    names.retain(|name| !name.is_empty() && !generic(name));
    names.sort();
    names.dedup();
    names
}

fn candidates(world: &ToolWorld<'_>) -> Vec<Candidate> {
    let mut candidates = BTreeMap::<PathBuf, Candidate>::new();
    let mut add = |path: &Path, label: Option<&str>| {
        let key = normalize(path);
        if let Some(candidate) = candidates.get_mut(&key) {
            candidate.aliases.extend(aliases(path, label));
        } else if candidates.len() < MAX_CANDIDATES {
            candidates.insert(
                key,
                Candidate {
                    path: path.to_path_buf(),
                    aliases: aliases(path, label),
                },
            );
        }
    };
    for entry in world.entries.iter().take(MAX_CANDIDATES) {
        add(&entry.spec.path, Some(entry.spec.label));
    }
    for finding in world.findings.iter().take(MAX_CANDIDATES) {
        if let Target::Cache(path) | Target::Folder(path) = &finding.target {
            add(path, None);
        }
    }
    if let Some(inventory) = world.inventory {
        for (path, children) in inventory.children.iter().take(MAX_CANDIDATES) {
            add(path, None);
            for child in children.iter().take(32) {
                add(&child.path, None);
            }
        }
        for item in inventory
            .top_level
            .iter()
            .chain(&inventory.largest)
            .take(MAX_CANDIDATES)
        {
            add(&item.path, None);
        }
    }
    candidates.into_values().collect()
}

fn explicit_paths(question: &str, home: &Path) -> Vec<PathBuf> {
    let mut pieces = Vec::new();
    let mut quoted = None;
    let mut piece = String::new();
    for c in question.chars() {
        if matches!(c, '"' | '`') {
            if quoted == Some(c) {
                pieces.push(std::mem::take(&mut piece));
                quoted = None;
            } else if quoted.is_none() {
                if !piece.is_empty() {
                    pieces.push(std::mem::take(&mut piece));
                }
                quoted = Some(c);
            } else {
                piece.push(c);
            }
        } else if c.is_whitespace() && quoted.is_none() {
            if !piece.is_empty() {
                pieces.push(std::mem::take(&mut piece));
            }
        } else {
            piece.push(c);
        }
    }
    if !piece.is_empty() {
        pieces.push(piece);
    }
    pieces
        .into_iter()
        .filter_map(|piece| {
            let piece = piece.trim_end_matches(['?', ',', ';', ':', ')']);
            if let Some(relative) = piece.strip_prefix("~/") {
                Some(home.join(relative))
            } else if piece.starts_with('/') {
                Some(PathBuf::from(piece))
            } else {
                None
            }
        })
        .collect()
}

fn measured_paths(question: &str, home: &Path, candidates: &[Candidate]) -> (Vec<PathBuf>, String) {
    let mut remaining = question.to_string();
    let mut paths = Vec::new();
    let mut ordered: Vec<_> = candidates.iter().collect();
    ordered.sort_by_key(|candidate| std::cmp::Reverse(candidate.path.as_os_str().len()));
    for candidate in ordered {
        let logical = normalize(&candidate.path);
        let mut forms = vec![
            candidate.path.to_string_lossy().into_owned(),
            logical.to_string_lossy().into_owned(),
        ];
        if let Ok(relative) = logical.strip_prefix("/") {
            forms.push(
                Path::new("/System/Volumes/Data")
                    .join(relative)
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        if let Ok(relative) = logical.strip_prefix(normalize(home)) {
            forms.push(format!("~/{}", relative.display()));
        }
        for form in forms {
            let spans: Vec<_> = remaining
                .match_indices(&form)
                .filter_map(|(start, _)| {
                    let end = start + form.len();
                    (remaining[..start]
                        .chars()
                        .next_back()
                        .is_none_or(|c| !c.is_alphanumeric())
                        && remaining[end..]
                            .chars()
                            .next()
                            .is_none_or(|c| !c.is_alphanumeric()))
                    .then_some((start, end))
                })
                .collect();
            if !spans.is_empty() {
                paths.push(candidate.path.clone());
                for (start, end) in spans.into_iter().rev() {
                    remaining.replace_range(start..end, " ");
                }
            }
        }
    }
    paths.extend(explicit_paths(&remaining, home));
    (paths, remaining)
}

fn unknown_name(question: &str) -> Option<String> {
    let words = words(question);
    for (index, word) in words.iter().enumerate() {
        if matches!(
            word.as_str(),
            "clean"
                | "cleanup"
                | "clear"
                | "remove"
                | "delete"
                | "inspect"
                | "app"
                | "folder"
                | "directory"
        ) && let Some(name) = words[index + 1..].iter().find(|name| !generic(name))
        {
            return Some(name.clone());
        }
    }
    None
}

fn scope(intent: Intent, target: Option<PathBuf>, world: &ToolWorld<'_>) -> QuestionScope {
    let label = target
        .as_deref()
        .map(|path| {
            let mut name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if generic(&name.to_lowercase()) {
                let components: Vec<_> = path
                    .components()
                    .filter_map(|part| part.as_os_str().to_str())
                    .collect();
                if let Some(pair) = components.windows(2).find(|pair| {
                    matches!(
                        pair[0],
                        "Application Support" | "Caches" | "Containers" | "Group Containers"
                    )
                }) {
                    name = pair[1];
                }
            }
            if name.starts_with("com.") || name.starts_with("org.") || name.starts_with("net.") {
                name = name.rsplit('.').next().unwrap_or(name);
            }
            let name = name.strip_suffix(".app").unwrap_or(name);
            if name.is_empty() {
                agent_tools::short_path(path, world.home)
            } else {
                ai::display_text(name).chars().take(80).collect()
            }
        })
        .unwrap_or_else(|| "Whole disk".into());
    QuestionScope {
        intent,
        target,
        label,
        clarification: None,
    }
}

/// Resolve intent without a model, filesystem probes, or unmeasured paths.
pub fn resolve(question: &str, selected: Option<&Path>, world: &ToolWorld<'_>) -> QuestionScope {
    let question = ai::display_text(question);
    let lower = question.to_lowercase();
    let words = words(&lower);
    let cleanup = agent_tools::is_cleanup_question(&question)
        || has(&words, &["legacy", "leftover", "leftovers", "uninstall"]);
    let intent = if cleanup {
        Intent::GlobalCleanup
    } else if has(
        &words,
        &[
            "grew",
            "grown",
            "growth",
            "bigger",
            "increased",
            "changed",
            "change",
        ],
    ) {
        Intent::Growth
    } else if has(
        &words,
        &[
            "process",
            "processes",
            "cpu",
            "memory",
            "ram",
            "running",
            "slow",
            "hang",
            "hung",
        ],
    ) {
        Intent::Processes
    } else if has(&words, &["disk", "space", "storage", "full", "capacity"]) {
        Intent::Capacity
    } else {
        Intent::Inspect
    };
    let target_intent = if cleanup {
        Intent::TargetCleanup
    } else if intent == Intent::Capacity {
        Intent::Inspect
    } else {
        intent
    };
    let candidates = candidates(world);
    let (paths, remaining) = measured_paths(&question, world.home, &candidates);
    let mut matches: Vec<_> = candidates
        .iter()
        .filter_map(|candidate| {
            let explicit = paths
                .iter()
                .any(|path| normalize(path) == normalize(&candidate.path));
            let match_len = candidate
                .aliases
                .iter()
                .filter(|alias| phrase_matches(&lower, alias))
                .map(String::len)
                .max();
            if !explicit && match_len.is_none() {
                return None;
            }
            let path = normalize(&candidate.path);
            let data = path
                .to_string_lossy()
                .contains("/Library/Application Support/") as usize
                * 30;
            let cache = (has(&words, &["cache", "caches"])
                && path.to_string_lossy().contains("/Library/Caches/"))
                as usize
                * 100;
            let home = path.starts_with(normalize(world.home)) as usize * 10;
            let depth = 32usize.saturating_sub(path.components().count());
            let score = if explicit {
                10_000
            } else {
                match_len.unwrap_or_default() * 100 + data + cache + home + depth
            };
            Some((score, candidate))
        })
        .collect();
    matches.sort_by(|(left_score, left), (right_score, right)| {
        right_score
            .cmp(left_score)
            .then_with(|| left.path.cmp(&right.path))
    });
    if !paths.is_empty()
        && !paths.iter().all(|path| {
            candidates
                .iter()
                .any(|candidate| normalize(&candidate.path) == normalize(path))
        })
    {
        let mut scope = scope(target_intent, None, world);
        scope.label = "Unresolved folder".into();
        scope.clarification = Some("Which measured folder do you mean? Open or select that exact folder and ask again; this path is not in the current inventory.".into());
        return scope;
    }
    if let Some(name) = unknown_name(&remaining)
        && !candidates.iter().any(|candidate| {
            candidate
                .aliases
                .iter()
                .any(|alias| phrase_matches(alias, &name))
        })
    {
        let mut scope = scope(target_intent, None, world);
        scope.label = ai::display_text(&name);
        scope.clarification = Some(format!(
            "Which measured app or folder is {}? It is not identified in the current inventory; select its exact folder and ask again.",
            scope.label
        ));
        return scope;
    }
    if let Some((score, candidate)) = matches.first() {
        let explicit_targets: BTreeSet<_> = paths.iter().map(|path| normalize(path)).collect();
        let mut owners: Vec<(&Candidate, &str)> = Vec::new();
        for (_, named) in &matches {
            if let Some(owner) = named
                .aliases
                .iter()
                .filter(|alias| phrase_matches(&lower, alias))
                .min_by_key(|alias| alias.len())
                && !owners.iter().any(|(known, name)| {
                    *name == owner
                        || normalize(&known.path).starts_with(normalize(&named.path))
                        || normalize(&named.path).starts_with(normalize(&known.path))
                })
            {
                owners.push((named, owner));
            }
        }
        if explicit_targets.len() > 1 || (explicit_targets.is_empty() && owners.len() > 1) {
            let mut scope = scope(target_intent, None, world);
            scope.label = "Multiple named targets".into();
            scope.clarification = Some("Which single app or folder should I inspect? Your question names multiple targets; select the exact item or include one measured path.".into());
            return scope;
        }
        let tied = matches.iter().skip(1).any(|(other_score, other)| {
            other_score == score
                && !normalize(&candidate.path).starts_with(normalize(&other.path))
                && !normalize(&other.path).starts_with(normalize(&candidate.path))
        });
        if tied {
            let mut scope = scope(target_intent, None, world);
            scope.label = "Ambiguous folder".into();
            scope.clarification = Some("Which app or folder do you mean? Select the exact item or include its measured path.".into());
            return scope;
        }
        let mut scope = scope(target_intent, Some(candidate.path.clone()), world);
        let remaining_lower = remaining.to_lowercase();
        if cleanup
            && (remaining_lower
                .split(|c: char| !c.is_alphanumeric())
                .any(|word| matches!(word, "not" | "never"))
                || phrase_matches(&remaining_lower, "don't")
                || phrase_matches(&remaining_lower, "don’t"))
        {
            scope.intent = Intent::Inspect;
            scope.clarification = Some(format!(
                "Your question excludes cleanup for {}. Would you like to inspect its storage instead?",
                scope.label
            ));
            return scope;
        }
        if cleanup
            && has(
                &words,
                &["legacy", "leftover", "leftovers", "old", "unused"],
            )
            && !has(&words, &["uninstall"])
            && !phrase_matches(&lower, "keep the app")
            && !phrase_matches(&lower, "keep using")
        {
            scope.clarification = Some(format!(
                "Do you want to keep the app and clear rebuildable caches, or remove the app and its data? Please clarify before cleaning {}.",
                scope.label
            ));
        }
        return scope;
    }
    if phrase_matches(&lower, "this folder")
        || phrase_matches(&lower, "this directory")
        || phrase_matches(&lower, "selected folder")
        || phrase_matches(&lower, "this app")
        || phrase_matches(&lower, "that app")
        || phrase_matches(&lower, "that folder")
    {
        let mut scope = scope(target_intent, selected.map(Path::to_path_buf), world);
        if selected.is_none() {
            scope.clarification =
                Some("Which folder do you mean? Select the exact item and ask again.".into());
        } else if cleanup
            && has(
                &words,
                &["legacy", "leftover", "leftovers", "old", "unused"],
            )
            && !has(&words, &["uninstall"])
            && !phrase_matches(&lower, "keep the app")
            && !phrase_matches(&lower, "keep using")
        {
            scope.clarification = Some(format!(
                "Do you want to keep the app and clear rebuildable caches, or remove the app and its data? Please clarify before cleaning {}.",
                scope.label
            ));
        }
        return scope;
    }
    if let Some(name) = unknown_name(&question) {
        let mut scope = scope(target_intent, None, world);
        scope.label = ai::display_text(&name);
        scope.clarification = Some(format!(
            "Which measured app or folder is {}? It is not identified in the current inventory; select its exact folder and ask again.",
            scope.label
        ));
        return scope;
    }
    if !cleanup
        && intent == Intent::Inspect
        && !has(
            &words,
            &[
                "folder",
                "directory",
                "files",
                "file",
                "data",
                "app",
                "application",
            ],
        )
    {
        let mut scope = scope(Intent::Unsupported, None, world);
        scope.label = "Unsupported question".into();
        scope.clarification = Some(
            "Ask about storage, a measured app or folder, cleanup, growth, or processes.".into(),
        );
        return scope;
    }
    scope(intent, None, world)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        cache::{CacheEntry, CacheStatus},
        care::Metrics,
        storage::{StorageCategory, StorageInventory, StorageItem, StorageItemKind},
    };

    fn inventory(home: &Path) -> StorageInventory {
        let cursor = home.join("Library/Application Support/Cursor");
        let item = |path: PathBuf| StorageItem {
            path,
            size_kb: 1024,
            kind: StorageItemKind::Directory,
            category: StorageCategory::ApplicationData,
        };
        let paths = [
            cursor.clone(),
            home.join("Library/Safari"),
            home.join("Documents/Café研究"),
            home.join("Library/Caches/pip"),
            home.join("Library/Application Support/Telegram"),
            home.join("code"),
        ];
        StorageInventory {
            volume: None,
            volume_error: None,
            roots: vec![],
            scanned_kb: 0,
            scanned_on_volume_kb: 0,
            unaccounted_kb: 0,
            inventory_overage_kb: 0,
            local_snapshots: vec![],
            scanned_items: 0,
            scan_errors: 0,
            scan_error_paths: vec![],
            complete: true,
            largest: vec![],
            top_level: paths.iter().cloned().map(item).collect(),
            children: paths
                .into_iter()
                .map(|path| (path, vec![]))
                .chain([(
                    cursor.clone(),
                    vec![item(cursor.join("User")), item(cursor.join("Cache"))],
                )])
                .collect(),
        }
    }

    fn world<'a>(
        home: &'a Path,
        inventory: Option<&'a StorageInventory>,
        entries: &'a [CacheEntry],
        metrics: &'a Metrics,
    ) -> ToolWorld<'a> {
        ToolWorld {
            home,
            inventory,
            entries,
            processes: &[],
            metrics,
            findings: &[],
            history: &[],
            current: None,
            volume: None,
            online_research: false,
            subject_pid: None,
            suggest: &|_| None,
        }
    }

    #[test]
    fn named_app_overrides_selection_and_capacity_is_target_inspection() {
        let home = Path::new("/Users/demo");
        let inventory = inventory(home);
        let metrics = Metrics::default();
        let world = world(home, Some(&inventory), &[], &metrics);
        let cursor = home.join("Library/Application Support/Cursor");
        let scoped = resolve("Can I safely clean Safari?", Some(&cursor), &world);
        assert_eq!(scoped.intent, Intent::TargetCleanup);
        assert_eq!(scoped.target, Some(home.join("Library/Safari")));
        assert_eq!(scoped.label, "Safari");
        assert!(!scoped.allows_path(&cursor));
        assert!(scoped.clarification.is_none());
        for question in [
            "What uses space in Cursor?",
            "What is using space in this folder?",
        ] {
            let scoped = resolve(question, Some(&cursor), &world);
            assert_eq!(scoped.intent, Intent::Inspect);
            assert_eq!(scoped.target, Some(cursor.clone()));
            assert_eq!(scoped.label, "Cursor");
        }
    }

    #[test]
    fn legacy_app_requires_keep_or_remove_choice_while_allowing_target_preflight() {
        let home = Path::new("/Users/demo");
        let inventory = inventory(home);
        let metrics = Metrics::default();
        let world = world(home, Some(&inventory), &[], &metrics);
        let cursor = home.join("Library/Application Support/Cursor");
        for question in ["Clean Cursor legacy files", "Clean old files for that app."] {
            let scoped = resolve(question, Some(&cursor), &world);
            assert_eq!(scoped.intent, Intent::TargetCleanup);
            assert_eq!(scoped.target, Some(cursor.clone()));
            assert_eq!(scoped.label, "Cursor");
            let text = scoped.clarification.as_deref().unwrap();
            assert!(text.contains("keep the app"));
            assert!(text.contains("remove the app"));
            assert!(scoped.allows_path(&cursor.join("User")));
            assert!(!scoped.allows_path(&home.join("Library/Caches/pip")));
        }
        assert!(
            resolve(
                "Clean Cursor legacy files; keep the app",
                Some(&cursor),
                &world
            )
            .clarification
            .is_none()
        );
        assert!(
            resolve("Clean old files for that app.", None, &world)
                .clarification
                .is_some()
        );
    }

    #[test]
    fn broad_cleanup_stays_global_despite_selection() {
        let home = Path::new("/Users/demo");
        let inventory = inventory(home);
        let metrics = Metrics::default();
        let world = world(home, Some(&inventory), &[], &metrics);
        let cursor = home.join("Library/Application Support/Cursor");
        for question in [
            "Free 10GB of disk space",
            "Which caches can I clear safely?",
            "Clean unused files",
        ] {
            let scoped = resolve(question, Some(&cursor), &world);
            assert_eq!(scoped.intent, Intent::GlobalCleanup, "{question}");
            assert!(scoped.target.is_none());
            assert!(!scoped.is_targeted());
            assert!(scoped.allows_path(&home.join("Library/Caches/pip")));
            assert!(scoped.clarification.is_none(), "{question}");
        }
    }

    #[test]
    fn contextual_folder_uses_exact_row_without_model_or_inventory() {
        let home = Path::new("/Users/demo");
        let metrics = Metrics::default();
        let world = world(home, None, &[], &metrics);
        let selected = home.join("code/project/target/debug");
        let scoped = resolve("Inspect this folder", Some(&selected), &world);
        assert_eq!(scoped.intent, Intent::Inspect);
        assert_eq!(scoped.target, Some(selected.clone()));
        assert!(!scoped.allows_path(selected.parent().unwrap()));
        assert!(scoped.is_targeted());
        assert!(
            resolve("Inspect this folder", None, &world)
                .clarification
                .is_some()
        );
    }

    #[test]
    fn partial_inventory_binds_known_apps_and_never_invents_unknown_paths() {
        let home = Path::new("/Users/demo");
        let mut inventory = inventory(home);
        inventory.complete = false;
        inventory.scan_errors = 3;
        let metrics = Metrics::default();
        let world = world(home, Some(&inventory), &[], &metrics);
        let cursor = home.join("Library/Application Support/Cursor");
        assert_eq!(
            resolve("Clean Safari", Some(&cursor), &world).target,
            Some(home.join("Library/Safari"))
        );
        for question in [
            "Clean FictionalApp caches",
            "Clean CursorClone caches",
            "Inspect ~/Library/UnknownApp",
        ] {
            let scoped = resolve(question, Some(&cursor), &world);
            assert!(scoped.target.is_none(), "{question}");
            assert!(scoped.clarification.is_some(), "{question}");
            assert!(!scoped.allows_path(&cursor));
            assert!(!scoped.allows_path(&home.join("Library/Caches/pip")));
        }
    }

    #[test]
    fn known_rule_labels_work_without_inventory_and_unicode_is_deterministic() {
        let home = Path::new("/Users/demo");
        let metrics = Metrics::default();
        let entries: Vec<_> = crate::cache::scan_specs(home, home)
            .into_iter()
            .filter(|spec| spec.label == "Safari cache")
            .map(|spec| CacheEntry {
                spec,
                status: CacheStatus::ScanError,
                size_kb: 0,
                identity: None,
                outcome: None,
            })
            .collect();
        let scoped = resolve(
            "Inspect Safari caches",
            None,
            &world(home, None, &entries, &metrics),
        );
        assert_eq!(
            scoped.target,
            Some(home.join("Library/Caches/com.apple.Safari"))
        );
        assert_eq!(scoped.label, "Safari");
        assert!(scoped.clarification.is_none());
        let inventory = inventory(home);
        let world = world(home, Some(&inventory), &[], &metrics);
        let first = resolve("Inspect CAFÉ研究", None, &world);
        assert_eq!(first, resolve("Inspect CAFÉ研究", None, &world));
        assert_eq!(first.target, Some(home.join("Documents/Café研究")));
        assert!(first.clarification.is_none());
    }

    #[test]
    fn explicit_paths_support_spaces_and_apfs_aliases_before_tokenization() {
        let home = Path::new("/Users/demo");
        let inventory = inventory(home);
        let metrics = Metrics::default();
        let world = world(home, Some(&inventory), &[], &metrics);
        let safari = home.join("Library/Safari");
        let cursor = home.join("Library/Application Support/Cursor");
        for question in [
            "Inspect ~/Library/Application Support/Cursor",
            "Inspect \"~/Library/Application Support/Cursor\"",
            "Inspect /System/Volumes/Data/Users/demo/Library/Application Support/Cursor",
        ] {
            let scoped = resolve(question, Some(&safari), &world);
            assert_eq!(
                scoped.target,
                Some(cursor.clone()),
                "{question}: {scoped:?}"
            );
            assert!(scoped.clarification.is_none());
        }
    }

    #[test]
    fn multiple_independently_named_owners_and_exclusions_clarify() {
        let home = Path::new("/Users/demo");
        let inventory = inventory(home);
        let metrics = Metrics::default();
        let world = world(home, Some(&inventory), &[], &metrics);
        for question in ["Clear pip, not Cursor", "Clean Cursor and Telegram"] {
            let scoped = resolve(question, None, &world);
            assert!(scoped.target.is_none(), "{question}");
            assert!(scoped.clarification.is_some());
            assert!(!scoped.allows_path(&home.join("Library/Caches/pip")));
        }
    }

    #[test]
    fn unknown_named_app_cannot_be_replaced_by_another_named_known_app() {
        let home = Path::new("/Users/demo");
        let inventory = inventory(home);
        let metrics = Metrics::default();
        let world = world(home, Some(&inventory), &[], &metrics);
        for question in [
            "Clean GhostApp and Cursor",
            "Clean GhostApp and ~/Library/Application Support/Cursor",
        ] {
            let scoped = resolve(question, None, &world);
            assert_eq!(scoped.intent, Intent::TargetCleanup);
            assert!(scoped.target.is_none());
            assert!(
                scoped
                    .clarification
                    .as_deref()
                    .unwrap()
                    .contains("ghostapp")
            );
            assert!(!scoped.allows_path(&home.join("Library/Application Support/Cursor")));
        }
    }

    #[test]
    fn negated_cleanup_is_inspection_with_clarification_and_no_cleanup_authority() {
        let home = Path::new("/Users/demo");
        let inventory = inventory(home);
        let metrics = Metrics::default();
        let world = world(home, Some(&inventory), &[], &metrics);
        let cursor = home.join("Library/Application Support/Cursor");
        for question in [
            "Do not delete Cursor",
            "Don't clean Cursor",
            "Never remove Cursor",
        ] {
            let scoped = resolve(question, None, &world);
            assert_eq!(scoped.intent, Intent::Inspect, "{question}");
            assert_eq!(scoped.target, Some(cursor.clone()));
            assert!(scoped.clarification.is_some());
            assert!(scoped.allows_path(&cursor));
            assert!(!scoped.allows_path(&home.join("Library/Caches/pip")));
        }
    }

    #[test]
    fn subtree_policy_rejects_prefix_confusion_and_parent_escapes() {
        let scoped = QuestionScope {
            intent: Intent::TargetCleanup,
            target: Some("/Users/demo/Library/Application Support/Cursor".into()),
            label: "Cursor".into(),
            clarification: None,
        };
        for path in [
            "/Users/demo/Library/Application Support/Cursor",
            "/Users/demo/Library/Application Support/Cursor/User",
            "/System/Volumes/Data/Users/demo/Library/Application Support/Cursor/Cache",
        ] {
            assert!(scoped.allows_path(Path::new(path)), "{path}");
        }
        for path in [
            "/Users/demo/Library/Application Support",
            "/Users/demo/Library/Application Support/CursorClone",
            "/Users/demo/Library/Application Support/Cursor/../Safari",
            "/System/Volumes/DataEscape/Users/demo/Library/Application Support/Cursor",
        ] {
            assert!(!scoped.allows_path(Path::new(path)), "{path}");
        }
    }

    #[test]
    fn whole_disk_questions_are_not_misread_as_unknown_folder_names() {
        let home = Path::new("/Users/demo");
        let inventory = inventory(home);
        let metrics = Metrics::default();
        let world = world(home, Some(&inventory), &[], &metrics);
        for (question, intent) in [
            ("What grew since last week?", Intent::Growth),
            ("Why is my disk full?", Intent::Capacity),
            (
                "Why doesn't the folder scan add up to the used space?",
                Intent::Capacity,
            ),
            ("Which processes use CPU?", Intent::Processes),
        ] {
            let scoped = resolve(question, Some(home), &world);
            assert_eq!(scoped.intent, intent, "{question}");
            assert!(scoped.target.is_none());
            assert!(scoped.clarification.is_none(), "{question}: {scoped:?}");
        }
        assert_eq!(
            resolve("What is using space in my code folder?", None, &world).target,
            Some(home.join("code"))
        );
        let scoped = resolve("Write me a poem about cats.", None, &world);
        assert_eq!(scoped.intent, Intent::Unsupported);
        assert!(!scoped.allows_path(home));
        assert_eq!(
            serde_json::to_string(&Intent::TargetCleanup).unwrap(),
            "\"target_cleanup\""
        );
        assert_eq!(
            serde_json::from_str::<QuestionScope>(&serde_json::to_string(&scoped).unwrap())
                .unwrap(),
            scoped
        );
    }
}
