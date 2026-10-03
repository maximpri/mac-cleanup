// SPDX-License-Identifier: GPL-3.0-or-later
//! Direct answers composed from collector-owned facts, never model prose.

use crate::{
    agent_tools, ai,
    cache::format_kb,
    investigation::{EvidenceRecord, EvidenceStatus, InvestigationCase},
    question::{Intent, QuestionScope},
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

const LIMIT: usize = 600;

pub fn summary(case: &InvestigationCase, cited: &[String], scope: &QuestionScope) -> String {
    if let Some(clarification) = &scope.clarification
        && scope.target.is_none()
    {
        return sentence(clarification);
    }
    let records: Vec<_> = case
        .evidence
        .iter()
        .filter(|record| cited.contains(&record.id))
        .collect();
    match scope.intent {
        Intent::TargetCleanup | Intent::Inspect if scope.target.is_some() => {
            let all: Vec<_> = case.evidence.iter().collect();
            targeted(&all, scope)
        }
        Intent::GlobalCleanup => global_cleanup(case, &records),
        Intent::Unsupported => "I can help with this Mac's storage and activity. Ask about a folder, cleanup, growth, disk capacity, or a process.".into(),
        Intent::Growth | Intent::Capacity | Intent::Processes => topical(&records, scope),
        _ => "Choose the folder you want to inspect so I can check that location.".into(),
    }
}

fn logical_path(path: &Path) -> PathBuf {
    path.strip_prefix("/System/Volumes/Data")
        .map(|suffix| Path::new("/").join(suffix))
        .unwrap_or_else(|_| path.to_path_buf())
}

fn label(scope: &QuestionScope) -> String {
    let label = ai::display_text(&scope.label);
    if label.is_empty() || label.chars().count() > 90 {
        "The requested folder".into()
    } else {
        label
    }
}

fn targeted(records: &[&EvidenceRecord], scope: &QuestionScope) -> String {
    let target = scope.target.as_deref().unwrap();
    let mut safety = BTreeMap::new();
    for record in records {
        if let Some(path) = &record.details.target
            && logical_path(path).starts_with(logical_path(target))
        {
            if record.details.open_handles.is_some() || record.scope.starts_with("open_handles(") {
                safety.insert((logical_path(path), "open"), *record);
            }
            if record.details.rule_status.is_some() || record.scope.starts_with("cleanup_rule(") {
                safety.insert((logical_path(path), "rule"), *record);
            }
        }
    }
    let positive_open = safety
        .iter()
        .filter(|((_, kind), record)| *kind == "open" && record.status.can_support_hypothesis())
        .filter_map(|(_, record)| record.details.open_handles.filter(|count| *count > 0))
        .max();
    let subtree_busy = positive_open.is_some()
        || safety.values().any(|record| {
            record.status.can_support_hypothesis()
                && record.details.rule_status.as_deref() == Some("IN USE")
        });
    let blocked = safety.iter().any(|((_, kind), record)| {
        record.status != EvidenceStatus::Complete
            || match *kind {
                "open" => record.details.open_handles != Some(0),
                _ => !matches!(
                    record.details.rule_status.as_deref(),
                    Some("READY" | "OPTIONAL")
                ),
            }
    });
    let records: Vec<_> = records
        .iter()
        .copied()
        .filter(|record| {
            record
                .details
                .target
                .as_deref()
                .is_some_and(|path| logical_path(path) == logical_path(target))
        })
        .collect();
    let name = label(scope);
    let measurement = records
        .iter()
        .rev()
        .find(|record| record.status.can_support_hypothesis() && record.details.size_kb.is_some());
    let children = records.iter().rev().find(|record| {
        record.status.can_support_hypothesis() && !record.details.children.is_empty()
    });
    // Failed later checks must not silently fall back to an older safety result.
    let rule = records.iter().rev().find(|record| {
        record.details.rule_status.is_some() || record.scope.starts_with("cleanup_rule(")
    });
    let owners = records.iter().rev().find(|record| {
        record.details.open_handles.is_some() || record.scope.starts_with("open_handles(")
    });
    let rule_status = rule
        .filter(|record| record.status.can_support_hypothesis())
        .and_then(|record| record.details.rule_status.as_deref());
    let open_count = positive_open.or_else(|| {
        owners
            .filter(|record| record.status == EvidenceStatus::Complete)
            .and_then(|record| record.details.open_handles)
    });
    let eligible = rule.is_some_and(|record| {
        record.status == EvidenceStatus::Complete
            && !blocked
            && record.details.eligible_actions.iter().any(|action| {
                action.strip_prefix("clean:").is_some_and(|path| {
                    logical_path(Path::new(path)).starts_with(logical_path(target))
                })
            })
    });
    let partial = records
        .iter()
        .any(|record| record.status == EvidenceStatus::Partial)
        || safety
            .values()
            .any(|record| record.status == EvidenceStatus::Partial);
    let failed_safety = safety
        .values()
        .any(|record| !record.status.can_support_hypothesis());
    let (conclusion, next) = if let Some(clarification) = &scope.clarification {
        (
            format!("These checks do not establish obsolete files in {name}."),
            sentence(clarification),
        )
    } else if scope.intent == Intent::Inspect {
        let conclusion = measurement
            .map(|record| {
                let size = format_kb(record.details.size_kb.unwrap());
                if record.status == EvidenceStatus::Partial {
                    format!("The partial scan observed at least {size} in {name}.")
                } else {
                    format!("The scan measured {size} in {name}.")
                }
            })
            .unwrap_or_else(|| format!("I could not measure {name} from these checks."));
        (
            conclusion,
            if partial || measurement.is_none() {
                "Run a complete scan of this folder to fill in the missing measurements.".into()
            } else {
                "Open its largest measured item to inspect the contents.".into()
            },
        )
    } else if subtree_busy {
        (
            format!("Keep {name} for now: files or a related application are in use."),
            "Close the related work, then repeat the safety checks before reviewing cleanup."
                .into(),
        )
    } else {
        match rule_status {
            Some("READY") if eligible && open_count == Some(0) => (
                format!("{name} is eligible for its reviewed cleanup."),
                "Review the listed cleanup action and its regeneration cost before confirming it.".into(),
            ),
            Some("OPTIONAL") if eligible && open_count == Some(0) => (
                format!("{name} is a reinstallable cache eligible for review."),
                "Review the cleanup action; restoring this cache may require a download.".into(),
            ),
            Some("PROTECTED") => (format!("{name} is protected from cleanup."),
                "Keep these files while the protection rule applies.".into()),
            Some("NO_RULE") => (format!("{name} has no approved cleanup rule."),
                "Inspect its contents and confirm how they can be restored before removing data.".into()),
            Some("PARTIAL_RULE") => (format!("Cleanup rules cover only part of {name}; clearing the whole folder is not verified."),
                "Review the specific subfolders covered by those rules.".into()),
            Some("REVIEW") => (format!("{name} contains app or personal data that needs review."),
                "Use the owning application's cleanup or migration process after reviewing the data.".into()),
            Some("SCAN ERROR" | "INVALID" | "SYMLINK") => (format!("{name} could not pass the cleanup safety checks."),
                "Resolve the access or path issue, then repeat the checks.".into()),
            _ => (format!("Cleanup safety for {name} is not confirmed yet."),
                "Complete the folder, cleanup-rule, and open-file checks before reviewing removal.".into()),
        }
    };
    let mut context = Vec::new();
    if partial {
        context.push("Some measurements are partial, so the folder may contain more data.".into());
    }
    if failed_safety {
        context.push("A cleanup or open-file check could not be completed.".into());
    }
    if let Some(count) = open_count {
        context.push(if count == 0 {
            "No process had files open there during the check.".into()
        } else {
            format!(
                "{count} process{} had files open there during the check.",
                if count == 1 { "" } else { "es" }
            )
        });
    } else if scope.intent == Intent::TargetCleanup {
        context.push("Open-file use has not been verified.".into());
    }
    if (scope.intent == Intent::TargetCleanup || scope.clarification.is_some())
        && let Some(record) = measurement
    {
        let size = format_kb(record.details.size_kb.unwrap());
        context.push(if record.status == EvidenceStatus::Partial {
            format!("At least {size} was observed in this folder.")
        } else {
            format!("The measured folder size is {size}.")
        });
    }
    if let Some(record) = children
        && let Some(text) = children_sentence(&record.details.children)
    {
        context.push(text);
    }
    compose(conclusion, context, next)
}

fn children_sentence(children: &[(String, u64)]) -> Option<String> {
    let mut children: Vec<_> = children.iter().collect();
    children.sort_by_key(|(_, size)| std::cmp::Reverse(*size));
    if children
        .first()
        .is_some_and(|(name, _)| name.is_empty() || name.chars().count() > 60)
    {
        return None;
    }
    let listed: Vec<_> = children
        .into_iter()
        .take(2)
        .filter(|(name, _)| !name.is_empty() && name.chars().count() <= 60)
        .map(|(name, size)| format!("“{}” ({})", ai::display_text(name), format_kb(*size)))
        .collect();
    match listed.as_slice() {
        [one] => Some(format!("The largest measured item is {one}.")),
        [one, two] => Some(format!("The largest measured items are {one} and {two}.")),
        _ => None,
    }
}

fn global_cleanup(case: &InvestigationCase, records: &[&EvidenceRecord]) -> String {
    let Some(overview) = records
        .iter()
        .rev()
        .find(|record| record.cleanup_total_kb.is_some() && record.status.can_support_hypothesis())
    else {
        return "The checks have not established a safe cleanup total. Finish the cleanup assessment to identify eligible items.".into();
    };
    let total = overview.cleanup_total_kb.unwrap();
    let goal = case
        .question
        .as_deref()
        .and_then(agent_tools::cleanup_goal_kb);
    let conclusion = match goal {
        Some(goal) if total < goal => format!(
            "The requested amount is not covered: eligible cleanup is {}, leaving at least {} still to find.",
            format_kb(total),
            format_kb(goal - total)
        ),
        Some(_) => format!(
            "Eligible cleanup totals {} and may cover the requested amount.",
            format_kb(total)
        ),
        None => format!(
            "Eligible cleanup totals {} before verifying actual freed space.",
            format_kb(total)
        ),
    };
    let mut context = Vec::new();
    if overview.status == EvidenceStatus::Partial {
        context.push("The scan is partial; other candidates may still be unmeasured.".into());
    }
    if let Some((name, size)) = overview.details.children.first()
        && name.chars().count() <= 100
    {
        context.push(format!(
            "Inspect “{}” ({}); its size does not establish that removal is safe.",
            ai::display_text(name),
            format_kb(*size)
        ));
    }
    compose(
        conclusion,
        context,
        "Review eligible actions, then verify the space actually freed.".into(),
    )
}

fn topical(records: &[&EvidenceRecord], scope: &QuestionScope) -> String {
    let relevant = records.iter().rev().find(|record| match scope.intent {
        Intent::Growth => {
            record.scope.starts_with("growth(") || record.scope.starts_with("growth_history(")
        }
        Intent::Capacity => record.scope.starts_with("disk_accounting("),
        Intent::Processes => record.kind == crate::investigation::EvidenceKind::ProcessSample,
        _ => false,
    });
    let Some(record) = relevant else {
        return match scope.intent {
            Intent::Growth => "Growth has not been established. Compare complete assessments of the same location to measure change.".into(),
            Intent::Capacity => "Capacity has not been established. Complete the volume accounting check to measure used and free space.".into(),
            _ => "Process activity has not been established. Take a current process sample to check the requested activity.".into(),
        };
    };
    let text = without_handles(&record.summary);
    // Collector text may already be capped. Retain only complete sentences from
    // such a result, rather than presenting a chopped measurement as an answer.
    let text = if text.ends_with('…') {
        text.rsplit_once(". ")
            .map(|(complete, _)| format!("{complete}."))
            .unwrap_or_default()
    } else {
        sentence(&text)
    };
    if text.is_empty() {
        return "The available check is incomplete. Repeat it to obtain a complete measurement."
            .into();
    }
    compose(text, vec![], String::new())
}

fn without_handles(text: &str) -> String {
    ai::display_text(text)
        .split_whitespace()
        .filter(|word| {
            let token = word.trim_matches(|character: char| !character.is_ascii_alphanumeric());
            !token.strip_prefix(['n', 'p', 'A']).is_some_and(|digits| {
                !digits.is_empty()
                    && digits.len() <= 2
                    && digits.chars().all(|character| character.is_ascii_digit())
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn sentence(text: &str) -> String {
    let text = ai::display_text(text);
    if text.is_empty() || text.ends_with(['.', '?', '!']) {
        text
    } else {
        format!("{text}.")
    }
}

fn compose(conclusion: String, context: Vec<String>, next: String) -> String {
    let mut sentences = vec![sentence(&conclusion)];
    let reserve = next.chars().count() + usize::from(!next.is_empty());
    for text in context {
        let text = sentence(&text);
        let used: usize = sentences
            .iter()
            .map(|sentence| sentence.chars().count() + 1)
            .sum();
        if used + text.chars().count() + reserve <= LIMIT {
            sentences.push(text);
        }
    }
    if !next.is_empty() {
        sentences.push(sentence(&next));
    }
    sentences.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::investigation::{self, EvidenceDetails, EvidenceKind};

    fn scope(intent: Intent, target: &str, label: &str) -> QuestionScope {
        QuestionScope {
            intent,
            target: Some(target.into()),
            label: label.into(),
            clarification: None,
        }
    }

    fn record(
        id: &str,
        tool: &str,
        target: &str,
        status: EvidenceStatus,
        details: EvidenceDetails,
    ) -> EvidenceRecord {
        let mut record = investigation::evidence(
            id,
            EvidenceKind::Policy,
            format!("{tool}({target})"),
            "Raw text n1 A1 must never become a targeted answer.",
            &[],
            &[],
            status,
        );
        record.details = EvidenceDetails {
            target: Some(target.into()),
            ..details
        };
        record
    }

    fn ready_case() -> InvestigationCase {
        let mut case = InvestigationCase::new_question("Can I clear pip?", 1);
        let target = "/Users/demo/Library/Caches/pip";
        case.evidence = vec![
            record(
                "E1",
                "list_children",
                target,
                EvidenceStatus::Complete,
                EvidenceDetails {
                    size_kb: Some(2048),
                    children: vec![("http".into(), 2048)],
                    ..EvidenceDetails::default()
                },
            ),
            record(
                "E2",
                "cleanup_rule",
                target,
                EvidenceStatus::Complete,
                EvidenceDetails {
                    rule_status: Some("READY".into()),
                    eligible_actions: vec![format!("clean:{target}")],
                    ..EvidenceDetails::default()
                },
            ),
            record(
                "E3",
                "open_handles",
                target,
                EvidenceStatus::Complete,
                EvidenceDetails {
                    open_handles: Some(0),
                    ..EvidenceDetails::default()
                },
            ),
        ];
        case
    }

    #[test]
    fn eligible_target_answer_leads_with_the_decision_and_uses_typed_facts() {
        let case = ready_case();
        let text = summary(
            &case,
            &["E1".into()],
            &scope(
                Intent::TargetCleanup,
                "/Users/demo/Library/Caches/pip",
                "pip",
            ),
        );
        assert!(
            text.starts_with("pip is eligible for its reviewed cleanup."),
            "{text}"
        );
        assert!(text.contains("2.0 MiB") && text.contains("“http”"));
        assert!(text.contains("No process had files open"));
        assert!(text.contains("Review the listed cleanup action"));
        assert!(!text.contains("n1") && !text.contains("A1") && !text.contains("Raw text"));
        assert!(text.chars().count() <= LIMIT && text.ends_with('.'));
    }

    #[test]
    fn unrelated_cache_evidence_cannot_answer_a_project_cleanup_question() {
        let mut case = ready_case();
        case.evidence.push(record(
            "E4",
            "cleanup_rule",
            "/Users/demo/Projects",
            EvidenceStatus::Complete,
            EvidenceDetails {
                rule_status: Some("NO_RULE".into()),
                ..EvidenceDetails::default()
            },
        ));
        case.evidence.push(record(
            "E5",
            "list_children",
            "/Users/demo/Projects",
            EvidenceStatus::Partial,
            EvidenceDetails {
                size_kb: Some(120 * 1024),
                children: vec![("target".into(), 120 * 1024)],
                ..EvidenceDetails::default()
            },
        ));
        let text = summary(
            &case,
            &["E1".into(), "E2".into()],
            &scope(Intent::TargetCleanup, "/Users/demo/Projects", "Projects"),
        );
        assert!(
            text.starts_with("Projects has no approved cleanup rule."),
            "{text}"
        );
        assert!(
            text.contains("120.0 MiB") && text.contains("“target”") && text.contains("partial")
        );
        assert!(!text.contains("pip") && !text.contains("2.0 MiB"));
    }

    #[test]
    fn uncited_busy_descendant_blocks_parent_cleanup_and_preserves_the_open_count() {
        let mut case = ready_case();
        case.evidence.push(record(
            "E4",
            "open_handles",
            "/Users/demo/Library/Caches/pip/http",
            EvidenceStatus::Partial,
            EvidenceDetails {
                open_handles: Some(2),
                ..EvidenceDetails::default()
            },
        ));
        let text = summary(
            &case,
            &["E1".into(), "E2".into(), "E3".into()],
            &scope(
                Intent::TargetCleanup,
                "/Users/demo/Library/Caches/pip",
                "pip",
            ),
        );
        assert!(text.starts_with("Keep pip for now"), "{text}");
        assert!(text.contains("2 processes") && !text.contains("No process had"));
        assert!(!text.contains("is eligible"));
    }

    #[test]
    fn failed_or_partial_clear_checks_do_not_establish_safe_cleanup() {
        for status in [
            EvidenceStatus::Partial,
            EvidenceStatus::Failed,
            EvidenceStatus::TimedOut,
        ] {
            let mut case = ready_case();
            case.evidence[2].status = status;
            let text = summary(
                &case,
                &["E2".into()],
                &scope(
                    Intent::TargetCleanup,
                    "/Users/demo/Library/Caches/pip",
                    "pip",
                ),
            );
            assert!(
                text.starts_with("Cleanup safety for pip is not confirmed yet."),
                "{status:?}: {text}"
            );
            assert!(!text.contains("No process had"));
        }
        let mut case = ready_case();
        case.evidence[1].status = EvidenceStatus::Partial;
        let text = summary(
            &case,
            &["E1".into()],
            &scope(
                Intent::TargetCleanup,
                "/Users/demo/Library/Caches/pip",
                "pip",
            ),
        );
        assert!(!text.contains("is eligible"), "{text}");
    }

    #[test]
    fn an_uncited_failed_child_check_blocks_an_older_parent_clear_result() {
        let mut case = ready_case();
        case.evidence.push(record(
            "E4",
            "open_handles",
            "/Users/demo/Library/Caches/pip/http",
            EvidenceStatus::TimedOut,
            EvidenceDetails::default(),
        ));
        let text = summary(
            &case,
            &["E2".into()],
            &scope(
                Intent::TargetCleanup,
                "/Users/demo/Library/Caches/pip",
                "pip",
            ),
        );
        assert!(
            text.starts_with("Cleanup safety for pip is not confirmed yet."),
            "{text}"
        );
        assert!(!text.contains("is eligible"));
    }

    #[test]
    fn a_later_complete_child_check_replaces_an_older_busy_observation() {
        let mut case = ready_case();
        let target = "/Users/demo/Library/Caches/pip/http";
        for (id, count) in [("E4", 1), ("E5", 0)] {
            case.evidence.push(record(
                id,
                "open_handles",
                target,
                EvidenceStatus::Complete,
                EvidenceDetails {
                    open_handles: Some(count),
                    ..EvidenceDetails::default()
                },
            ));
        }
        let text = summary(
            &case,
            &["E4".into()],
            &scope(
                Intent::TargetCleanup,
                "/Users/demo/Library/Caches/pip",
                "pip",
            ),
        );
        assert!(text.starts_with("pip is eligible"), "{text}");
    }

    #[test]
    fn legacy_clarification_keeps_measurements_and_does_not_infer_obsolescence() {
        let mut case = ready_case();
        case.evidence[0].status = EvidenceStatus::Partial;
        let mut scope = scope(
            Intent::TargetCleanup,
            "/Users/demo/Library/Caches/pip",
            "pip",
        );
        scope.clarification = Some("Which app or artifact do you mean by legacy files?".into());
        let text = summary(&case, &["E2".into()], &scope);
        assert!(
            text.starts_with("These checks do not establish obsolete files in pip."),
            "{text}"
        );
        assert!(text.contains("2.0 MiB") && text.contains("partial"));
        assert!(text.ends_with("Which app or artifact do you mean by legacy files?"));
        assert!(!text.contains("is eligible"));
        assert!(text.chars().count() <= LIMIT);
    }

    #[test]
    fn inspection_describes_partial_measurements_without_a_cleanup_verdict() {
        let mut case = ready_case();
        case.evidence[0].status = EvidenceStatus::Partial;
        let text = summary(
            &case,
            &["E1".into()],
            &scope(Intent::Inspect, "/Users/demo/Library/Caches/pip", "pip"),
        );
        assert!(
            text.starts_with("The partial scan observed at least 2.0 MiB in pip."),
            "{text}"
        );
        assert!(text.contains("“http”") && text.contains("Run a complete scan"));
        assert!(!text.contains("eligible") && !text.contains("cleanup rule"));
    }

    #[test]
    fn global_cleanup_leads_with_total_and_shortfall_without_handles() {
        let mut case = InvestigationCase::new_question("Free 10 GiB of disk space", 1);
        let mut record = investigation::evidence(
            "E1",
            EvidenceKind::Policy,
            "cleanup_options()",
            "Raw aggregate n1 A1",
            &[],
            &[],
            EvidenceStatus::Partial,
        );
        record.cleanup_total_kb = Some(8 * 1024);
        record.details.children = vec![("~/code/project/target".into(), 120 * 1024)];
        case.evidence.push(record);
        let scope = QuestionScope {
            intent: Intent::GlobalCleanup,
            target: None,
            label: "disk space".into(),
            clarification: None,
        };
        let text = summary(&case, &["E1".into()], &scope);
        assert!(
            text.starts_with("The requested amount is not covered: eligible cleanup is 8.0 MiB"),
            "{text}"
        );
        assert!(text.contains("target") && text.contains("partial"));
        assert!(!text.contains("n1") && !text.contains("A1") && !text.contains("Raw aggregate"));
    }

    #[test]
    fn long_measurement_labels_are_omitted_whole_without_losing_the_next_step() {
        let mut case = ready_case();
        case.evidence[0].details.children = vec![("🧰".repeat(1000), 2048)];
        let text = summary(
            &case,
            &["E1".into()],
            &scope(
                Intent::TargetCleanup,
                "/Users/demo/Library/Caches/pip",
                &"🧹".repeat(200),
            ),
        );
        assert!(text.chars().count() <= LIMIT);
        assert!(!text.contains('…') && !text.contains('🧰'));
        assert!(text.ends_with(
            "Review the listed cleanup action and its regeneration cost before confirming it."
        ));
    }

    #[test]
    fn data_volume_aliases_preserve_target_measurements() {
        let case = ready_case();
        let text = summary(
            &case,
            &["E1".into()],
            &scope(
                Intent::Inspect,
                "/System/Volumes/Data/Users/demo/Library/Caches/pip",
                "pip",
            ),
        );
        assert!(text.contains("2.0 MiB") && text.contains("“http”"));
    }
}
