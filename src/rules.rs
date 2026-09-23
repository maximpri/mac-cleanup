// SPDX-License-Identifier: GPL-3.0-or-later
//! Cleanup rules loaded from TOML rule packs.
//!
//! Built-in packs are compiled into the binary. Users can disable a bundled
//! pack (`disabled_packs` in settings.json) or add their own packs under
//! `~/.config/diskray/rules/*.toml`. Every pack is validated before any rule
//! becomes a cleanup target:
//!
//! - paths are relative to the account home, with no `..`, `~`, globs, or
//!   control characters, and no two rules may overlap;
//! - a native command is only a reference to a compiled-in command whose
//!   pinned path must equal the rule's path, so a pack can never point a
//!   vendor command somewhere else;
//! - review-tier rules can never be quick wins or run native commands.
//!
//! Rules are parsed once per process; their strings live for the whole run.

use crate::cache::{CacheSpec, CacheTarget, CacheTier};
use serde::Deserialize;
use std::{
    collections::HashSet,
    fs,
    path::{Component, Path},
    sync::OnceLock,
};

/// A compiled-in vendor command a rule may reference by `id`.
#[derive(Debug)]
pub struct Native {
    pub id: &'static str,
    /// The command as shown to the user.
    pub display: &'static str,
    /// The only rule path this command may be used for.
    pub path: &'static str,
}

pub const NATIVE: &[Native] = &[
    Native {
        id: "pip",
        display: "python3 -m pip cache purge",
        path: "Library/Caches/pip",
    },
    Native {
        id: "go",
        display: "go clean -cache -testcache -fuzzcache",
        path: "Library/Caches/go-build",
    },
    Native {
        id: "uv",
        display: "uv cache clean",
        path: ".cache/uv",
    },
    Native {
        id: "yarn",
        display: "yarn cache clean",
        path: "Library/Caches/Yarn",
    },
    Native {
        id: "playwright",
        display: "playwright uninstall --all",
        path: "Library/Caches/ms-playwright",
    },
    Native {
        id: "cypress",
        display: "cypress cache clear",
        path: "Library/Caches/Cypress",
    },
    Native {
        id: "huggingface",
        display: "hf cache prune --yes",
        path: ".cache/huggingface/hub",
    },
];

/// Built-in packs in load order: core first, then bundled optional packs.
pub const BUILTIN_PACKS: &[(&str, &str)] = &[
    ("rules/core.toml", include_str!("../rules/core.toml")),
    (
        "rules/apps/telegram.toml",
        include_str!("../rules/apps/telegram.toml"),
    ),
    (
        "rules/apps/tradingview.toml",
        include_str!("../rules/apps/tradingview.toml"),
    ),
    (
        "rules/apps/zcode.toml",
        include_str!("../rules/apps/zcode.toml"),
    ),
    (
        "rules/apps/google.toml",
        include_str!("../rules/apps/google.toml"),
    ),
    (
        "rules/apps/adobe.toml",
        include_str!("../rules/apps/adobe.toml"),
    ),
    (
        "rules/dev/ai-tools.toml",
        include_str!("../rules/dev/ai-tools.toml"),
    ),
    (
        "rules/dev/orbstack.toml",
        include_str!("../rules/dev/orbstack.toml"),
    ),
    (
        "rules/dev/playwright-go.toml",
        include_str!("../rules/dev/playwright-go.toml"),
    ),
];

#[derive(Debug, Clone, Copy)]
pub struct Guidance {
    pub classification: &'static str,
    pub delete_scope: &'static str,
    pub impact: &'static str,
    pub recovery: &'static str,
    pub recommendation: &'static str,
}

/// One validated rule.
#[derive(Debug, Clone)]
pub struct Rule {
    pub pack: &'static str,
    pub id: &'static str,
    pub label: &'static str,
    /// Relative to the account home.
    pub path: &'static str,
    pub target: CacheTarget,
    pub tier: CacheTier,
    /// `pgrep -f` pattern for apps that mark the data in use.
    pub process_pattern: &'static str,
    pub note: &'static str,
    pub quick_win: bool,
    pub native: Option<&'static Native>,
    /// Lower-case command substrings that relate a running process to the rule.
    pub associate: Vec<&'static str>,
    pub advice: Option<&'static str>,
    pub guidance: Option<Guidance>,
    /// Loaded from the user's own rule pack.
    pub user: bool,
}

impl Rule {
    pub fn full_id(&self) -> String {
        format!("{}.{}", self.pack, self.id)
    }

    pub fn spec(&self, root: &Path) -> CacheSpec {
        CacheSpec {
            home: root.to_path_buf(),
            path: root.join(self.path),
            label: self.label,
            tier: self.tier,
            process_pattern: self.process_pattern,
            note: self.note,
            target: self.target,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PackFile {
    schema: u32,
    pack: String,
    #[serde(default)]
    #[allow(dead_code)]
    description: String,
    #[serde(default)]
    rule: Vec<RuleFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleFile {
    id: String,
    label: String,
    path: String,
    #[serde(default = "default_target")]
    target: String,
    min_age_days: Option<u64>,
    tier: String,
    #[serde(default)]
    processes: Vec<String>,
    note: String,
    #[serde(default)]
    quick_win: bool,
    native: Option<String>,
    #[serde(default)]
    associate: Vec<String>,
    advice: Option<String>,
    guidance: Option<GuidanceFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GuidanceFile {
    classification: String,
    delete_scope: String,
    impact: String,
    recovery: String,
    recommendation: String,
}

fn default_target() -> String {
    "contents".into()
}

fn leak(text: String) -> &'static str {
    String::leak(text)
}

fn plain_text(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

fn valid_id(value: &str) -> bool {
    (1..=40).contains(&value.len())
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// A path is valid when it is a plain relative path inside the home.
pub fn valid_relative_path(value: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > 512 {
        return Err("path must be 1–512 bytes".into());
    }
    if value
        .chars()
        .any(|c| c.is_control() || matches!(c, '*' | '?' | '[' | ']' | '{' | '}'))
    {
        return Err("path may not contain control or glob characters".into());
    }
    if value.starts_with('~') {
        return Err("path must be relative to the home, without `~`".into());
    }
    let path = Path::new(value);
    if path.is_absolute() {
        return Err("path must be relative to the home".into());
    }
    if !path
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err("path may not contain `.`, `..`, or empty segments".into());
    }
    if value.contains("//") || value.ends_with('/') {
        return Err("path may not contain empty segments".into());
    }
    Ok(())
}

/// Each `processes` entry is one alternative; `|` is allowed only inside a
/// group such as `(install|download)`.
fn has_top_level_alternation(pattern: &str) -> bool {
    let mut depth = 0_i32;
    let mut escaped = false;
    for c in pattern.chars() {
        match c {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '(' => depth += 1,
            ')' => depth -= 1,
            '|' if depth <= 0 => return true,
            _ => {}
        }
    }
    false
}

fn overlaps(left: &str, right: &str) -> bool {
    let left = Path::new(left);
    let right = Path::new(right);
    left.starts_with(right) || right.starts_with(left)
}

fn validate_rule(pack: &str, rule: RuleFile, user: bool) -> Result<Rule, String> {
    let name = format!("{pack}.{}", rule.id);
    let fail = |reason: String| Err(format!("{name}: {reason}"));
    if !valid_id(&rule.id) {
        return fail("id must be 1–40 lower-case letters, digits, or dashes".into());
    }
    if !plain_text(&rule.label, 60) {
        return fail("label must be 1–60 characters of plain text".into());
    }
    if !plain_text(&rule.note, 300) {
        return fail("note must be 1–300 characters of plain text".into());
    }
    if let Err(reason) = valid_relative_path(&rule.path) {
        return fail(reason);
    }
    let tier = match rule.tier.as_str() {
        "routine" => CacheTier::Routine,
        "reinstallable" => CacheTier::Reinstallable,
        "review" => CacheTier::ReviewOnly,
        other => return fail(format!("unknown tier `{other}`")),
    };
    let target = match (rule.target.as_str(), rule.min_age_days) {
        ("contents", None) => CacheTarget::DirectoryContents,
        ("aged", Some(days)) if (1..=3650).contains(&days) => {
            CacheTarget::AgedContents { min_age_days: days }
        }
        ("aged", _) => return fail("aged rules need min_age_days between 1 and 3650".into()),
        ("contents", Some(_)) => return fail("min_age_days applies only to aged rules".into()),
        (other, _) => return fail(format!("unknown target `{other}`")),
    };
    if tier == CacheTier::ReviewOnly && (rule.quick_win || rule.native.is_some()) {
        return fail("review-tier rules cannot be quick wins or run native commands".into());
    }
    if user && rule.native.is_some() {
        return fail("user rule packs cannot run native commands".into());
    }
    let native = match &rule.native {
        None => None,
        Some(id) => {
            let Some(native) = NATIVE.iter().find(|native| native.id == id) else {
                return fail(format!("unknown native command `{id}`"));
            };
            if native.path != rule.path || target != CacheTarget::DirectoryContents {
                return fail(format!(
                    "native command `{id}` is pinned to {} and cannot be used here",
                    native.path
                ));
            }
            Some(native)
        }
    };
    if rule.processes.len() > 12
        || rule
            .processes
            .iter()
            .any(|pattern| !plain_text(pattern, 200) || has_top_level_alternation(pattern))
    {
        return fail(
            "processes must be at most 12 patterns of up to 200 characters, one alternative each"
                .into(),
        );
    }
    if rule.associate.iter().any(|value| !plain_text(value, 100)) {
        return fail("associate entries must be short plain text".into());
    }
    if rule
        .advice
        .as_deref()
        .is_some_and(|advice| !plain_text(advice, 400))
    {
        return fail("advice must be plain text up to 400 characters".into());
    }
    let guidance = match rule.guidance {
        None => None,
        Some(g) => {
            if [
                &g.classification,
                &g.delete_scope,
                &g.impact,
                &g.recovery,
                &g.recommendation,
            ]
            .iter()
            .any(|text| !plain_text(text, 300))
            {
                return fail("guidance fields must be plain text up to 300 characters".into());
            }
            Some(Guidance {
                classification: leak(g.classification),
                delete_scope: leak(g.delete_scope),
                impact: leak(g.impact),
                recovery: leak(g.recovery),
                recommendation: leak(g.recommendation),
            })
        }
    };
    Ok(Rule {
        pack: leak(pack.to_string()),
        id: leak(rule.id),
        label: leak(rule.label),
        path: leak(rule.path),
        target,
        tier,
        process_pattern: leak(rule.processes.join("|")),
        note: leak(rule.note),
        quick_win: rule.quick_win,
        native,
        associate: rule
            .associate
            .into_iter()
            .map(|value| leak(value.to_lowercase()))
            .collect(),
        advice: rule.advice.map(leak),
        guidance,
        user,
    })
}

/// Parse one pack. A structural error rejects the whole pack; an invalid rule
/// is reported and skipped.
pub fn parse_pack(text: &str, user: bool) -> Result<(String, Vec<Rule>, Vec<String>), String> {
    let file: PackFile = toml::from_str(text).map_err(|error| error.to_string())?;
    if file.schema != 1 {
        return Err(format!("unsupported schema {}", file.schema));
    }
    if !valid_id(&file.pack) {
        return Err("pack name must be 1–40 lower-case letters, digits, or dashes".into());
    }
    let mut rules = Vec::new();
    let mut problems = Vec::new();
    let mut ids = HashSet::new();
    for rule in file.rule {
        if !ids.insert(rule.id.clone()) {
            problems.push(format!("{}.{}: duplicate rule id", file.pack, rule.id));
            continue;
        }
        match validate_rule(&file.pack, rule, user) {
            Ok(rule) => rules.push(rule),
            Err(problem) => problems.push(problem),
        }
    }
    Ok((file.pack, rules, problems))
}

/// Rules from several packs, with conflicts resolved: earlier packs win and a
/// later rule whose path overlaps an existing one is dropped.
#[derive(Debug, Default)]
pub struct RuleSet {
    pub rules: Vec<Rule>,
    pub warnings: Vec<String>,
}

impl RuleSet {
    fn add_pack(&mut self, source: &str, text: &str, user: bool, disabled: &[String]) {
        match parse_pack(text, user) {
            Err(error) => self.warnings.push(format!("{source}: {error}")),
            Ok((pack, _, _)) if disabled.iter().any(|name| name == &pack) => {}
            Ok((pack, rules, problems)) => {
                if self.rules.iter().any(|rule| rule.pack == pack) {
                    self.warnings
                        .push(format!("{source}: pack `{pack}` is already loaded"));
                    return;
                }
                self.warnings.extend(problems);
                for rule in rules {
                    if let Some(existing) = self
                        .rules
                        .iter()
                        .find(|existing| overlaps(existing.path, rule.path))
                    {
                        self.warnings.push(format!(
                            "{}: overlaps {} ({}) and was skipped",
                            rule.full_id(),
                            existing.full_id(),
                            existing.path
                        ));
                        continue;
                    }
                    self.rules.push(rule);
                }
            }
        }
    }

    pub fn by_label(&self, label: &str) -> Option<&Rule> {
        self.rules.iter().find(|rule| rule.label == label)
    }
}

/// The compiled-in packs only, with nothing disabled.
pub fn builtin() -> &'static RuleSet {
    static BUILTIN: OnceLock<RuleSet> = OnceLock::new();
    BUILTIN.get_or_init(|| {
        let mut set = RuleSet::default();
        for (source, text) in BUILTIN_PACKS {
            set.add_pack(source, text, false, &[]);
        }
        set
    })
}

/// The rules in effect for this account: bundled packs minus disabled ones,
/// plus the user's own packs. Tests always use the built-in set.
pub fn active() -> &'static RuleSet {
    #[cfg(test)]
    {
        builtin()
    }
    #[cfg(not(test))]
    {
        static ACTIVE: OnceLock<RuleSet> = OnceLock::new();
        ACTIVE.get_or_init(|| match crate::cache::account_home() {
            Some(home) => load_for(&home),
            None => {
                let mut set = RuleSet::default();
                for (source, text) in BUILTIN_PACKS {
                    set.add_pack(source, text, false, &[]);
                }
                set
            }
        })
    }
}

/// Load bundled and user packs for an account home.
pub fn load_for(home: &Path) -> RuleSet {
    let disabled = crate::care::disabled_packs(home);
    let mut set = RuleSet::default();
    for (source, text) in BUILTIN_PACKS {
        set.add_pack(source, text, false, &disabled);
    }
    let directory = home.join(crate::paths::CONFIG_SUBPATH).join("rules");
    let Ok(metadata) = fs::symlink_metadata(&directory) else {
        return set;
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        set.warnings.push(format!(
            "{} is not a real directory; user rules were not loaded",
            directory.display()
        ));
        return set;
    }
    let mut files: Vec<_> = fs::read_dir(&directory)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    for path in files.into_iter().take(50) {
        let readable = fs::symlink_metadata(&path)
            .is_ok_and(|m| m.is_file() && !m.file_type().is_symlink() && m.len() <= 256 * 1024);
        match readable.then(|| fs::read_to_string(&path).ok()).flatten() {
            Some(text) => set.add_pack(&path.display().to_string(), &text, true, &disabled),
            None => set
                .warnings
                .push(format!("{}: not a readable regular file", path.display())),
        }
    }
    set
}

/// Specs for every active rule under an account home.
pub fn home_specs(root: &Path) -> Vec<CacheSpec> {
    active().rules.iter().map(|rule| rule.spec(root)).collect()
}

/// The rule a cleanup entry came from, by its label.
pub fn for_label(label: &str) -> Option<&'static Rule> {
    active()
        .by_label(label)
        .or_else(|| builtin().by_label(label))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_pack_is_valid() {
        for (source, text) in BUILTIN_PACKS {
            let (pack, rules, problems) = parse_pack(text, false).unwrap();
            assert!(problems.is_empty(), "{source}: {problems:?}");
            assert!(!rules.is_empty(), "{pack} has no rules");
        }
        assert!(builtin().warnings.is_empty(), "{:?}", builtin().warnings);
    }

    /// The rules that existed before packs, as (path, label, tier, target).
    #[test]
    fn builtin_packs_keep_every_legacy_rule() {
        let legacy: &[(&str, &str, CacheTier)] = &[
            (".Trash", "Trash", CacheTier::Routine),
            ("Library/Caches/pip", "pip cache", CacheTier::Routine),
            (
                "Library/Caches/node-gyp",
                "node-gyp cache",
                CacheTier::Routine,
            ),
            (
                "Library/Caches/Homebrew",
                "Homebrew cache",
                CacheTier::Routine,
            ),
            (
                "Library/Caches/com.apple.python",
                "Python cache",
                CacheTier::Routine,
            ),
            (
                "Library/Caches/tradingview-desktop-updater",
                "TradingView updater",
                CacheTier::Routine,
            ),
            (
                "Library/Caches/@zcodedesktop-updater",
                "ZCode updater",
                CacheTier::Routine,
            ),
            (
                "Library/Caches/ru.keepcoder.Telegram",
                "Telegram cache",
                CacheTier::Routine,
            ),
            (
                "Library/Caches/Google",
                "Google app cache",
                CacheTier::Routine,
            ),
            (
                "Library/Caches/com.apple.Safari",
                "Safari cache",
                CacheTier::Routine,
            ),
            (
                "Library/Caches/Firefox",
                "Firefox cache",
                CacheTier::Routine,
            ),
            (
                "Library/Caches/Adobe",
                "Adobe app cache",
                CacheTier::Routine,
            ),
            (".npm/_cacache", "npm package cache", CacheTier::Routine),
            (".cache/opencode", "OpenCode cache", CacheTier::Routine),
            (
                "Library/Caches/go-build",
                "Go build cache",
                CacheTier::Routine,
            ),
            (".cache/uv", "uv package cache", CacheTier::Routine),
            (
                "Library/Caches/Yarn",
                "Yarn package cache",
                CacheTier::Routine,
            ),
            (
                "Library/Developer/Xcode/DerivedData",
                "Xcode derived data",
                CacheTier::Routine,
            ),
            (
                "Library/Developer/CoreSimulator/Caches",
                "Simulator cache",
                CacheTier::Routine,
            ),
            (
                "Library/Caches/org.swift.swiftpm",
                "SwiftPM cache",
                CacheTier::Reinstallable,
            ),
            (
                "Library/Caches/ms-playwright",
                "Playwright browsers",
                CacheTier::Reinstallable,
            ),
            (
                "Library/Caches/ms-playwright-go",
                "Playwright Go browsers",
                CacheTier::Reinstallable,
            ),
            (".npm/_npx", "npm npx packages", CacheTier::Reinstallable),
            (
                ".cache/chrome-devtools-mcp",
                "Chrome DevTools MCP profile",
                CacheTier::ReviewOnly,
            ),
            (
                ".cache/codex-runtimes",
                "Codex runtimes",
                CacheTier::Reinstallable,
            ),
            (".gradle/caches", "Gradle caches", CacheTier::Reinstallable),
            (
                "Library/Caches/CocoaPods",
                "CocoaPods cache",
                CacheTier::Reinstallable,
            ),
            (
                "Library/Caches/Cypress",
                "Cypress runtimes",
                CacheTier::Reinstallable,
            ),
            (
                ".cache/huggingface/hub",
                "Hugging Face models",
                CacheTier::Reinstallable,
            ),
            (
                "Library/Developer/Xcode/iOS DeviceSupport",
                "Xcode device support",
                CacheTier::ReviewOnly,
            ),
            (
                "Library/Developer/Xcode/Archives",
                "Xcode archives",
                CacheTier::ReviewOnly,
            ),
            (
                "Library/Developer/CoreSimulator/Devices",
                "Simulator devices",
                CacheTier::ReviewOnly,
            ),
            (
                "Library/Application Support/Cursor/User",
                "Cursor user data",
                CacheTier::ReviewOnly,
            ),
            (
                "Library/Group Containers/HUAQ24HBR6.dev.orbstack/data",
                "OrbStack data",
                CacheTier::ReviewOnly,
            ),
            (
                "Library/Group Containers/6N38VWS5BX.ru.keepcoder.Telegram/stable",
                "Telegram local data",
                CacheTier::ReviewOnly,
            ),
            ("Library/Logs", "User logs", CacheTier::Routine),
            (
                "Library/Saved Application State",
                "Saved app state",
                CacheTier::Routine,
            ),
            (
                "Library/DiagnosticReports",
                "Crash reports",
                CacheTier::Routine,
            ),
        ];
        let rules = &builtin().rules;
        assert_eq!(rules.len(), legacy.len());
        for (path, label, tier) in legacy {
            let rule = rules
                .iter()
                .find(|rule| rule.path == *path)
                .unwrap_or_else(|| panic!("missing {path}"));
            assert_eq!(rule.label, *label);
            assert_eq!(rule.tier, *tier, "{label}");
        }
        let aged: Vec<_> = rules
            .iter()
            .filter_map(|rule| match rule.target {
                CacheTarget::AgedContents { min_age_days } => Some((rule.label, min_age_days)),
                _ => None,
            })
            .collect();
        assert_eq!(
            aged,
            vec![
                ("User logs", 7),
                ("Saved app state", 30),
                ("Crash reports", 30)
            ]
        );
        let quick: HashSet<_> = rules
            .iter()
            .filter(|r| r.quick_win)
            .map(|r| r.label)
            .collect();
        assert_eq!(quick.len(), 8);
        assert_eq!(
            rules.iter().filter(|r| r.native.is_some()).count(),
            NATIVE.len()
        );
    }

    fn pack(rule: &str) -> String {
        format!("schema = 1\npack = \"mine\"\n[[rule]]\n{rule}\n")
    }

    #[test]
    fn invalid_rules_are_rejected_with_a_reason() {
        let base = "id = \"x\"\nlabel = \"X\"\ntier = \"routine\"\nnote = \"n\"\n";
        for (extra, expected) in [
            ("path = \"/etc\"", "relative"),
            ("path = \"~/Library\"", "without `~`"),
            ("path = \"Library/../..\"", "`..`"),
            ("path = \"Library/*\"", "glob"),
            (
                "path = \"a\"\nnative = \"pip\"",
                "user rule packs cannot run native",
            ),
            ("path = \"a\"\ntier2 = 1", "unknown field"),
        ] {
            let text = pack(&format!("{base}{extra}"));
            let outcome = parse_pack(&text, true);
            let message = match outcome {
                Err(error) => error,
                Ok((_, rules, problems)) => {
                    assert!(rules.is_empty(), "{extra} should be rejected");
                    problems.join(" ")
                }
            };
            assert!(message.contains(expected), "{extra}: {message}");
        }
        let review = pack(
            "id = \"r\"\nlabel = \"R\"\npath = \"a\"\ntier = \"review\"\nnote = \"n\"\nquick_win = true",
        );
        assert!(parse_pack(&review, false).unwrap().2[0].contains("review-tier"));
        let redirected = pack(
            "id = \"p\"\nlabel = \"P\"\npath = \"elsewhere\"\ntier = \"routine\"\nnote = \"n\"\nnative = \"go\"",
        );
        assert!(parse_pack(&redirected, false).unwrap().2[0].contains("pinned"));
        let aged = pack(
            "id = \"a\"\nlabel = \"A\"\npath = \"a\"\ntarget = \"aged\"\ntier = \"routine\"\nnote = \"n\"",
        );
        assert!(parse_pack(&aged, false).unwrap().2[0].contains("min_age_days"));
        let piped = pack(
            "id = \"q\"\nlabel = \"Q\"\npath = \"a\"\ntier = \"routine\"\nnote = \"n\"\nprocesses = [\"a|b\"]",
        );
        assert!(parse_pack(&piped, false).unwrap().2[0].contains("one alternative"));
    }

    #[test]
    fn user_packs_load_but_cannot_override_or_overlap_builtin_rules() {
        let home = tempfile::tempdir().unwrap();
        let directory = home.path().join(".config/diskray/rules");
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("mine.toml"),
            pack("id = \"notes\"\nlabel = \"Notes cache\"\npath = \"Library/Caches/com.example.notes\"\ntier = \"routine\"\nnote = \"rebuilt\"\n[[rule]]\nid = \"pip-inner\"\nlabel = \"Inner\"\npath = \"Library/Caches/pip/wheels\"\ntier = \"routine\"\nnote = \"x\""),
        )
        .unwrap();
        fs::write(directory.join("broken.toml"), "schema = 1\npack = ").unwrap();
        let set = load_for(home.path());
        let notes = set
            .rules
            .iter()
            .find(|rule| rule.label == "Notes cache")
            .unwrap();
        assert!(notes.user);
        assert!(!set.rules.iter().any(|rule| rule.label == "Inner"));
        assert!(
            set.warnings
                .iter()
                .any(|warning| warning.contains("overlaps core.pip"))
        );
        assert!(
            set.warnings
                .iter()
                .any(|warning| warning.contains("broken.toml"))
        );
    }

    #[test]
    fn bundled_packs_can_be_disabled() {
        let home = tempfile::tempdir().unwrap();
        crate::care::set_disabled_packs(home.path(), &["telegram".into(), "adobe".into()]).unwrap();
        let set = load_for(home.path());
        assert!(
            !set.rules
                .iter()
                .any(|rule| rule.pack == "telegram" || rule.pack == "adobe")
        );
        assert!(set.rules.iter().any(|rule| rule.pack == "core"));
    }
}
