//! User-editable protection list for cleanup targets.
//!
//! A whitelist file at `~/.config/mac-cleanup/whitelist` protects locations
//! from every cleanup path, including ordinary cleanup and confirmed review
//! deletion. Each non-empty, non-comment line is one path pattern:
//!
//! - relative patterns resolve against the account home directory,
//! - a leading `~/` also resolves against the account home directory,
//! - `*` matches any characters within one path component and `?` matches one
//!   character,
//! - matching is case-insensitive, and a matching directory also protects
//!   everything inside it.

use std::{fs, path::Path, path::PathBuf};

const WHITELIST_SUBPATH: &str = ".config/mac-cleanup/whitelist";

pub fn whitelist_path(account_home: &Path) -> PathBuf {
    account_home.join(WHITELIST_SUBPATH)
}

/// The parsed contents of the user whitelist file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Whitelist {
    patterns: Vec<PathPattern>,
    source: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PathPattern {
    segments: Vec<String>,
}

impl Whitelist {
    pub fn empty() -> Self {
        Self {
            patterns: Vec::new(),
            source: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    pub fn pattern_count(&self) -> usize {
        self.patterns.len()
    }

    pub fn source_path(&self) -> Option<&Path> {
        self.source.as_deref()
    }

    /// Load the whitelist for an account home. A missing file is the normal
    /// case and yields an empty whitelist.
    pub fn load(account_home: &Path) -> Self {
        let path = whitelist_path(account_home);
        let text = fs::read_to_string(&path).unwrap_or_default();
        Self::parse(&text, account_home).with_source(path)
    }

    fn with_source(mut self, path: PathBuf) -> Self {
        self.source = Some(path);
        self
    }

    pub fn parse(text: &str, account_home: &Path) -> Self {
        let mut patterns = Vec::new();
        for line in text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
        {
            let expanded = expand_home(line, account_home);
            let absolute = if expanded.is_absolute() {
                expanded
            } else {
                account_home.join(expanded)
            };
            // A location can be reached through symlinked directories (for
            // example `/tmp` versus `/private/tmp`), so the resolved spelling
            // of the pattern is protected as well.
            patterns.extend(PathPattern::from_absolute(&absolute));
            if let Ok(resolved) = absolute.canonicalize()
                && resolved != absolute
            {
                patterns.extend(PathPattern::from_absolute(&resolved));
            }
        }
        Self {
            patterns,
            source: None,
        }
    }

    /// Whether the path or any of its parent directories matches a pattern.
    pub fn protects(&self, path: &Path) -> bool {
        if self.patterns.is_empty() {
            return false;
        }
        if self.matches_path_or_ancestor(path) {
            return true;
        }
        // Scan roots are canonicalized before cleanup targets are built, so a
        // candidate may spell its path differently from the whitelist entry.
        if let Ok(resolved) = path.canonicalize()
            && resolved != path
        {
            return self.matches_path_or_ancestor(&resolved);
        }
        false
    }

    fn matches_path_or_ancestor(&self, path: &Path) -> bool {
        let components: Vec<String> = path
            .components()
            .filter_map(|component| match component {
                std::path::Component::Normal(part) => {
                    let text = part.to_string_lossy().to_lowercase();
                    (!text.is_empty()).then_some(text)
                }
                _ => None,
            })
            .collect();
        if components.is_empty() {
            return false;
        }
        (1..=components.len()).any(|length| {
            let candidate = &components[..length];
            self.patterns
                .iter()
                .any(|pattern| pattern.matches(candidate))
        })
    }
}

impl PathPattern {
    fn from_absolute(absolute: &Path) -> Option<Self> {
        let segments: Vec<String> = absolute
            .components()
            .filter_map(|component| match component {
                std::path::Component::Normal(part) => {
                    let text = part.to_string_lossy().to_lowercase();
                    (!text.is_empty()).then_some(text)
                }
                _ => None,
            })
            .collect();
        (!segments.is_empty()).then_some(Self { segments })
    }

    fn matches(&self, candidate: &[String]) -> bool {
        candidate.len() == self.segments.len()
            && candidate
                .iter()
                .zip(&self.segments)
                .all(|(text, pattern)| segment_matches(pattern, text))
    }
}

fn expand_home(line: &str, account_home: &Path) -> PathBuf {
    if line == "~" {
        return account_home.to_path_buf();
    }
    if let Some(rest) = line.strip_prefix("~/") {
        return account_home.join(rest);
    }
    PathBuf::from(line)
}

/// Wildcard match within one path component: `*` spans any number of
/// characters, `?` spans exactly one.
fn segment_matches(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    let (mut pattern_index, mut text_index) = (0_usize, 0_usize);
    let (mut star_index, mut star_text) = (None::<usize>, 0_usize);
    while text_index < text.len() {
        if pattern_index < pattern.len()
            && (pattern[pattern_index] == '?' || pattern[pattern_index] == text[text_index])
        {
            pattern_index += 1;
            text_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == '*' {
            star_index = Some(pattern_index);
            star_text = text_index;
            pattern_index += 1;
        } else if let Some(star) = star_index {
            pattern_index = star + 1;
            star_text += 1;
            text_index = star_text;
        } else {
            return false;
        }
    }
    while pattern_index < pattern.len() && pattern[pattern_index] == '*' {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        PathBuf::from("/Users/tester")
    }

    #[test]
    fn empty_whitelist_protects_nothing() {
        let whitelist = Whitelist::empty();
        assert!(whitelist.is_empty());
        assert!(!whitelist.protects(&home().join("Library/Caches/pip")));
    }

    #[test]
    fn parses_comments_blank_lines_and_tilde_expansion() {
        let whitelist = Whitelist::parse(
            "# protected\n\n~/Library/Logs\nLibrary/Caches/Adobe\n",
            &home(),
        );
        assert_eq!(whitelist.patterns.len(), 2);
        assert!(whitelist.protects(&home().join("Library/Logs/app/x.log")));
        assert!(whitelist.protects(&home().join("Library/Caches/Adobe")));
        assert!(!whitelist.protects(&home().join("Library/Caches/pip")));
    }

    #[test]
    fn absolute_patterns_apply_outside_the_account_home() {
        let whitelist = Whitelist::parse("/private/tmp/keep-me", &home());
        assert!(whitelist.protects(Path::new("/private/tmp/keep-me")));
        assert!(whitelist.protects(Path::new("/private/tmp/keep-me/inner")));
        assert!(!whitelist.protects(Path::new("/private/tmp/other")));
    }

    #[test]
    fn a_matching_directory_protects_its_contents() {
        let whitelist = Whitelist::parse("Library/Developer", &home());
        assert!(
            whitelist.protects(
                &home().join("Library/Developer/Xcode/DerivedData/app-abc/Build/objects.o")
            )
        );
    }

    #[test]
    fn wildcards_match_within_one_component_only() {
        let whitelist = Whitelist::parse("Library/Caches/com.*", &home());
        assert!(whitelist.protects(&home().join("Library/Caches/com.apple.helpd")));
        assert!(!whitelist.protects(&home().join("Library/Caches/deep/nested")));

        let single = Whitelist::parse("Library/Caches/com.?????", &home());
        assert!(single.protects(&home().join("Library/Caches/com.pip01")));
        assert!(!single.protects(&home().join("Library/Caches/com.pip010")));
    }

    #[test]
    fn matching_is_case_insensitive() {
        let whitelist = Whitelist::parse("library/caches/adobe", &home());
        assert!(whitelist.protects(&home().join("Library/Caches/Adobe")));
    }

    #[test]
    fn symlinked_spellings_protect_the_resolved_location() {
        let temp = tempfile::tempdir().unwrap();
        let actual = temp.path().join("actual");
        let linked = temp.path().join("linked");
        fs::create_dir_all(actual.join("nested")).unwrap();
        std::os::unix::fs::symlink(&actual, &linked).unwrap();

        // A pattern spelled with the symlinked prefix must also protect the
        // canonical location it resolves to.
        let whitelist = Whitelist::parse(&format!("{}/nested", linked.display()), temp.path());
        let resolved = temp.path().canonicalize().expect("tempdir exists");
        let canonical_candidate = resolved.join("actual").join("nested");
        assert!(whitelist.protects(&canonical_candidate));
        assert!(whitelist.protects(&canonical_candidate.join("deep/file")));
        assert!(!whitelist.protects(&resolved.join("other/nested")));
    }

    #[test]
    fn malformed_lines_are_ignored() {
        let whitelist = Whitelist::parse("/\n///\n   \n#x\n", &home());
        assert!(whitelist.is_empty());
    }
}
