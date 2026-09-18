//! Bounded, evidence-driven investigations.
//!
//! The model may choose which supported check is useful next, but this module
//! owns the case lifecycle, evidence provenance, and resource budget. A case
//! can therefore explain an unresolved cause without turning a plausible story
//! into a claim of fact.

use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const AUTOMATIC_DECISION_BUDGET: u8 = 3;
pub const EXPLICIT_DECISION_BUDGET: u8 = 8;
pub const AUTOMATIC_CASE_WINDOW: Duration = Duration::from_secs(60);
pub const EXPLICIT_CASE_WINDOW: Duration = Duration::from_secs(180);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CasePhase {
    Observing,
    Checking,
    Researching,
    AwaitingApproval,
    Verifying,
    Complete,
    Inconclusive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    ProcessSample,
    FilesystemActivity,
    VolumeContext,
    Research,
    Experiment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HypothesisStatus {
    Open,
    Leading,
    Weakened,
    Supported,
    Unresolved,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceRecord {
    pub id: String,
    pub kind: EvidenceKind,
    pub observed_at: u64,
    pub scope: String,
    pub source: Option<String>,
    pub summary: String,
    pub complete: bool,
    pub supports: Vec<String>,
    pub contradicts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hypothesis {
    pub id: String,
    pub label: String,
    pub status: HypothesisStatus,
    pub rationale: String,
    pub supporting_evidence: Vec<String>,
    pub contradicting_evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvestigationCase {
    pub id: String,
    pub target: String,
    pub started_at: u64,
    pub revision: u64,
    pub automatic: bool,
    pub phase: CasePhase,
    pub decision_count: u8,
    pub decision_budget: u8,
    pub hypotheses: Vec<Hypothesis>,
    pub evidence: Vec<EvidenceRecord>,
    pub checks_run: Vec<String>,
    pub conclusion: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentDecision {
    Check {
        check: String,
        reason: String,
    },
    Research {
        topic: String,
        reason: String,
    },
    AwaitApproval {
        experiment: String,
        reason: String,
    },
    Finish {
        conclusion: String,
        phase: CasePhase,
    },
}

impl InvestigationCase {
    pub fn new_fseventsd(target: impl Into<String>, revision: u64, automatic: bool) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            id: format!("investigation:fseventsd:{now}"),
            target: target.into(),
            started_at: now,
            revision,
            automatic,
            phase: CasePhase::Observing,
            decision_count: 0,
            decision_budget: if automatic {
                AUTOMATIC_DECISION_BUDGET
            } else {
                EXPLICIT_DECISION_BUDGET
            },
            hypotheses: vec![
                hypothesis(
                    "filesystem_activity",
                    "A workload is producing a large filesystem event stream",
                    "Activity tracing can identify repeated writes, scans, or paging-related work.",
                ),
                hypothesis(
                    "volume_specific",
                    "One mounted volume or filesystem is contributing disproportionately",
                    "Volume context can separate the startup volume from external or custom filesystems.",
                ),
                hypothesis(
                    "daemon_or_history",
                    "fseventsd or its event history is behaving abnormally",
                    "This remains a fallback hypothesis until activity and volume evidence fail to explain the pattern.",
                ),
            ],
            evidence: Vec::new(),
            checks_run: Vec::new(),
            conclusion: None,
        }
    }

    pub fn add_evidence(&mut self, evidence: EvidenceRecord) {
        for hypothesis in &mut self.hypotheses {
            if evidence.supports.iter().any(|id| id == &hypothesis.id) {
                hypothesis.supporting_evidence.push(evidence.id.clone());
                if hypothesis.status == HypothesisStatus::Open {
                    hypothesis.status = HypothesisStatus::Leading;
                }
            }
            if evidence.contradicts.iter().any(|id| id == &hypothesis.id) {
                hypothesis.contradicting_evidence.push(evidence.id.clone());
                hypothesis.status = HypothesisStatus::Weakened;
            }
        }
        self.evidence.push(evidence);
        self.phase = CasePhase::Checking;
    }

    pub fn record_check(&mut self, check: impl Into<String>) {
        let check = check.into();
        if !self.checks_run.iter().any(|known| known == &check) {
            self.checks_run.push(check);
            self.decision_count = self.decision_count.saturating_add(1);
        }
    }

    pub fn next_local_decision(&self) -> AgentDecision {
        if self.decision_count >= self.decision_budget {
            return AgentDecision::Finish {
                conclusion: "The investigation budget is exhausted. The measured evidence identifies the leading area, but does not prove a root cause.".into(),
                phase: CasePhase::Inconclusive,
            };
        }
        if self.target.eq_ignore_ascii_case("fseventsd") {
            if !self.has_check("fs_usage") {
                return AgentDecision::Check {
                    check: "fs_usage".into(),
                    reason: "Separate fseventsd memory correlation from the filesystem activity it is observing.".into(),
                };
            }
            if !self.has_check("volume_context") {
                return AgentDecision::Check {
                    check: "volume_context".into(),
                    reason: "Identify whether activity is concentrated on an external or custom mounted volume.".into(),
                };
            }
            if !self.has_check("research_sources") {
                return AgentDecision::Research {
                    topic: "fseventsd memory growth filesystem event backlog macOS".into(),
                    reason: "Compare the observed pattern with documented platform behavior and known reports.".into(),
                };
            }
        }
        AgentDecision::Finish {
            conclusion: self.conclusion_text(),
            phase: CasePhase::Inconclusive,
        }
    }

    pub fn apply_decision(&mut self, decision: &AgentDecision) {
        self.phase = match decision {
            AgentDecision::Check { .. } => CasePhase::Checking,
            AgentDecision::Research { .. } => CasePhase::Researching,
            AgentDecision::AwaitApproval { .. } => CasePhase::AwaitingApproval,
            AgentDecision::Finish { conclusion, phase } => {
                self.conclusion = Some(conclusion.clone());
                *phase
            }
        };
    }

    pub fn has_check(&self, check: &str) -> bool {
        self.checks_run.iter().any(|known| known == check)
    }

    pub fn conclusion_text(&self) -> String {
        let supported = self
            .hypotheses
            .iter()
            .filter(|hypothesis| {
                matches!(
                    hypothesis.status,
                    HypothesisStatus::Leading | HypothesisStatus::Supported
                )
            })
            .map(|hypothesis| hypothesis.label.as_str())
            .collect::<Vec<_>>();
        if supported.is_empty() {
            "No single cause is supported by the available evidence yet.".into()
        } else {
            format!(
                "Leading explanation: {}. Further isolation is needed before calling it the root cause.",
                supported.join("; ")
            )
        }
    }
}

fn hypothesis(id: &str, label: &str, rationale: &str) -> Hypothesis {
    Hypothesis {
        id: id.into(),
        label: label.into(),
        status: HypothesisStatus::Open,
        rationale: rationale.into(),
        supporting_evidence: Vec::new(),
        contradicting_evidence: Vec::new(),
    }
}

pub fn evidence(
    id: impl Into<String>,
    kind: EvidenceKind,
    scope: impl Into<String>,
    summary: impl Into<String>,
    supports: &[&str],
    contradicts: &[&str],
    complete: bool,
) -> EvidenceRecord {
    EvidenceRecord {
        id: id.into(),
        kind,
        observed_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        scope: scope.into(),
        source: None,
        summary: summary.into(),
        complete,
        supports: supports.iter().map(|item| (*item).into()).collect(),
        contradicts: contradicts.iter().map(|item| (*item).into()).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fseventsd_case_chooses_distinguishing_checks_in_order() {
        let mut case = InvestigationCase::new_fseventsd("fseventsd", 4, false);
        assert!(matches!(
            case.next_local_decision(),
            AgentDecision::Check { ref check, .. } if check == "fs_usage"
        ));
        case.record_check("fs_usage");
        assert!(matches!(
            case.next_local_decision(),
            AgentDecision::Check { ref check, .. } if check == "volume_context"
        ));
        case.record_check("volume_context");
        assert!(matches!(
            case.next_local_decision(),
            AgentDecision::Research { ref topic, .. } if topic.contains("fseventsd")
        ));
    }

    #[test]
    fn evidence_updates_hypothesis_status_without_claiming_causation() {
        let mut case = InvestigationCase::new_fseventsd("fseventsd", 1, true);
        case.add_evidence(evidence(
            "fs-1",
            EvidenceKind::FilesystemActivity,
            "fseventsd",
            "Repeated swap-backed filesystem reads observed.",
            &["filesystem_activity"],
            &[],
            true,
        ));
        assert_eq!(case.hypotheses[0].status, HypothesisStatus::Leading);
        assert!(case.conclusion.is_none());
    }

    #[test]
    fn automatic_budget_finishes_as_inconclusive() {
        let mut case = InvestigationCase::new_fseventsd("fseventsd", 1, true);
        case.record_check("fs_usage");
        case.record_check("volume_context");
        case.record_check("research_sources");
        assert!(matches!(
            case.next_local_decision(),
            AgentDecision::Finish {
                phase: CasePhase::Inconclusive,
                ..
            }
        ));
    }
}
