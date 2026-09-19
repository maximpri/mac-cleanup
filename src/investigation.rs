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
    Policy,
    ProcessSample,
    FilesystemActivity,
    VolumeContext,
    Research,
    Experiment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvestigationFamily {
    CapacityCoverage,
    StorageGrowth,
    MemoryPressure,
    CpuActivity,
    FilesystemActivity,
    DeveloperOwnership,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceStatus {
    Complete,
    Partial,
    PermissionRequired,
    Unsupported,
    TimedOut,
    Cancelled,
    Failed,
}

impl EvidenceStatus {
    pub fn can_support_hypothesis(self) -> bool {
        matches!(self, Self::Complete | Self::Partial)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Partial => "partial",
            Self::PermissionRequired => "permission required",
            Self::Unsupported => "unsupported",
            Self::TimedOut => "timed out",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }
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
    pub status: EvidenceStatus,
    pub supports: Vec<String>,
    pub contradicts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckObservation {
    pub kind: EvidenceKind,
    pub status: EvidenceStatus,
    pub summary: String,
    pub source: Option<String>,
    pub supports: Vec<String>,
    pub contradicts: Vec<String>,
}

impl CheckObservation {
    pub fn complete(kind: EvidenceKind, summary: impl Into<String>) -> Self {
        Self {
            kind,
            status: EvidenceStatus::Complete,
            summary: summary.into(),
            source: None,
            supports: Vec::new(),
            contradicts: Vec::new(),
        }
    }

    pub fn unavailable(
        kind: EvidenceKind,
        status: EvidenceStatus,
        summary: impl Into<String>,
    ) -> Self {
        debug_assert!(!status.can_support_hypothesis());
        Self {
            kind,
            status,
            summary: summary.into(),
            source: None,
            supports: Vec::new(),
            contradicts: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvailableCheck {
    pub id: String,
    pub question: String,
    pub requires_approval: bool,
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
    pub family: InvestigationFamily,
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
        #[serde(default)]
        evidence_ids: Vec<String>,
        #[serde(default)]
        hypothesis_ids: Vec<String>,
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
        #[serde(default)]
        evidence_ids: Vec<String>,
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
            family: InvestigationFamily::FilesystemActivity,
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

    pub fn new_storage(target: impl Into<String>, revision: u64, automatic: bool) -> Self {
        let mut case = Self::base(
            target,
            revision,
            automatic,
            InvestigationFamily::StorageGrowth,
        );
        case.hypotheses = vec![
            hypothesis(
                "measured_children",
                "A small number of measured children explains most of this folder",
                "A complete child inventory can locate the space without treating size as waste.",
            ),
            hypothesis(
                "rebuildable_data",
                "The space is rebuildable or disposable data",
                "A known cleanup rule and inactive state are required before suggesting cleanup.",
            ),
            hypothesis(
                "active_writer",
                "An active workload is creating or retaining the data",
                "Open handles and comparable history can establish activity without claiming ownership.",
            ),
        ];
        case
    }

    pub fn new_developer(target: impl Into<String>, revision: u64, automatic: bool) -> Self {
        let mut case = Self::base(
            target,
            revision,
            automatic,
            InvestigationFamily::DeveloperOwnership,
        );
        case.hypotheses = vec![
            hypothesis(
                "measured_children",
                "Measured project, runtime, or cache children explain the footprint",
                "Child measurements identify the large components without assuming they are disposable.",
            ),
            hypothesis(
                "rebuildable_data",
                "A known developer cache or runtime is rebuildable",
                "Only an existing cleanup policy can establish a supported cleanup decision.",
            ),
            hypothesis(
                "active_writer",
                "A running developer workload still owns or uses this data",
                "Open-handle and current-process evidence can prevent cleanup while work is active.",
            ),
        ];
        case
    }

    pub fn new_process(
        target: impl Into<String>,
        revision: u64,
        automatic: bool,
        memory: bool,
    ) -> Self {
        let mut case = Self::base(
            target,
            revision,
            automatic,
            if memory {
                InvestigationFamily::MemoryPressure
            } else {
                InvestigationFamily::CpuActivity
            },
        );
        case.hypotheses = vec![
            hypothesis(
                "process_persists",
                "The exact process identity remains present across samples",
                "A refreshed identity can establish persistence, while intent and causation remain unknown.",
            ),
            hypothesis(
                "abnormal_process",
                "The process is stalled, repeatedly restarting, or behaving abnormally",
                "Identity, lifecycle, and repeated samples are needed before proposing a process action.",
            ),
            hypothesis(
                "system_pressure_correlation",
                "The process activity coincides with system pressure",
                "Correlation is useful context but does not establish how much memory or performance an action recovers.",
            ),
        ];
        case
    }

    pub fn new_capacity(revision: u64, automatic: bool) -> Self {
        let mut case = Self::base(
            "storage capacity",
            revision,
            automatic,
            InvestigationFamily::CapacityCoverage,
        );
        case.hypotheses = vec![
            hypothesis(
                "measured_consumers",
                "The directory inventory identifies the largest measured consumers",
                "A complete child inventory locates measured space; coverage evidence separately accounts for missing capacity.",
            ),
            hypothesis(
                "missing_coverage",
                "Protected paths, snapshots, or other volumes explain missing capacity",
                "Coverage and volume accounting must be reconciled before recommending action.",
            ),
        ];
        case
    }

    fn base(
        target: impl Into<String>,
        revision: u64,
        automatic: bool,
        family: InvestigationFamily,
    ) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            id: format!("investigation:{family:?}:{now}"),
            target: target.into(),
            family,
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
            hypotheses: Vec::new(),
            evidence: Vec::new(),
            checks_run: Vec::new(),
            conclusion: None,
        }
    }

    pub fn add_evidence(&mut self, evidence: EvidenceRecord) {
        if evidence.status.can_support_hypothesis() {
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
        }
        self.evidence.push(evidence);
        self.phase = CasePhase::Checking;
    }

    /// Attach only relationships that the deterministic collector can establish.
    /// Unknown relationship IDs are removed before evidence is persisted or sent
    /// to the model.
    pub fn classify_observation(&self, check: &str, observation: &mut CheckObservation) {
        if observation.status.can_support_hypothesis() {
            match (self.family, check) {
                (
                    InvestigationFamily::StorageGrowth | InvestigationFamily::DeveloperOwnership,
                    "inspect_children",
                ) => observation.supports.push("measured_children".into()),
                (InvestigationFamily::CapacityCoverage, "inspect_children") => {
                    observation.supports.push("measured_consumers".into());
                }
                (InvestigationFamily::CapacityCoverage, "volume_context")
                    if observation
                        .supports
                        .iter()
                        .any(|id| id == "volume_specific") =>
                {
                    observation.supports.push("missing_coverage".into());
                }
                _ => {}
            }
        }
        let known = self
            .hypotheses
            .iter()
            .map(|hypothesis| hypothesis.id.as_str())
            .collect::<std::collections::HashSet<_>>();
        observation
            .supports
            .retain(|id| known.contains(id.as_str()));
        observation
            .contradicts
            .retain(|id| known.contains(id.as_str()));
        observation.supports.sort();
        observation.supports.dedup();
        observation.contradicts.sort();
        observation.contradicts.dedup();
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
                evidence_ids: self.supporting_evidence_ids(),
            };
        }
        if self.target.eq_ignore_ascii_case("fseventsd") {
            if !self.has_check("fs_usage") {
                return AgentDecision::Check {
                    check: "fs_usage".into(),
                    reason: "Separate fseventsd memory correlation from the filesystem activity it is observing.".into(),
                    evidence_ids: self.supporting_evidence_ids(),
                    hypothesis_ids: vec!["filesystem_activity".into()],
                };
            }
            if !self.has_check("volume_context") {
                return AgentDecision::Check {
                    check: "volume_context".into(),
                    reason: "Identify whether activity is concentrated on an external or custom mounted volume.".into(),
                    evidence_ids: self.supporting_evidence_ids(),
                    hypothesis_ids: vec!["volume_specific".into()],
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
            evidence_ids: self.supporting_evidence_ids(),
        }
    }

    pub fn available_checks(&self, research_enabled: bool) -> Vec<AvailableCheck> {
        let mut checks = Vec::new();
        if self.target.eq_ignore_ascii_case("fseventsd") {
            if !self.has_check("fs_usage") {
                checks.push(AvailableCheck {
                    id: "fs_usage".into(),
                    question: "Is fseventsd handling ordinary paging, file activity, or sustained filesystem operations?".into(),
                    requires_approval: true,
                });
            }
            if !self.has_check("volume_context") {
                checks.push(AvailableCheck {
                    id: "volume_context".into(),
                    question:
                        "Are external or custom filesystems mounted while the symptom is present?"
                            .into(),
                    requires_approval: false,
                });
            }
            if research_enabled && !self.has_check("research_sources") {
                checks.push(AvailableCheck {
                    id: "research_sources".into(),
                    question: "What does current Apple documentation say that applies to this observation?".into(),
                    requires_approval: false,
                });
            }
        }
        match self.family {
            InvestigationFamily::StorageGrowth | InvestigationFamily::DeveloperOwnership => {
                if !self.has_check("inspect_children") {
                    checks.push(AvailableCheck {
                        id: "inspect_children".into(),
                        question: "Which direct children account for this space?".into(),
                        requires_approval: false,
                    });
                }
                if !self.has_check("check_open_handles") {
                    checks.push(AvailableCheck {
                        id: "check_open_handles".into(),
                        question: "Is an active process holding this location open?".into(),
                        requires_approval: false,
                    });
                }
                if !self.has_check("compare_history") {
                    checks.push(AvailableCheck {
                        id: "compare_history".into(),
                        question: "Do comparable complete measurements show growth or regrowth?"
                            .into(),
                        requires_approval: false,
                    });
                }
            }
            InvestigationFamily::MemoryPressure | InvestigationFamily::CpuActivity => {
                if !self.has_check("refresh_processes") {
                    checks.push(AvailableCheck {
                        id: "refresh_processes".into(),
                        question: "Is the exact process identity still present and active?".into(),
                        requires_approval: false,
                    });
                }
            }
            InvestigationFamily::CapacityCoverage => {
                if !self.has_check("inspect_children") {
                    checks.push(AvailableCheck {
                        id: "inspect_children".into(),
                        question: "Which measured children account for used storage?".into(),
                        requires_approval: false,
                    });
                }
                if !self.has_check("volume_context") {
                    checks.push(AvailableCheck {
                        id: "volume_context".into(),
                        question: "Does APFS or mounted-volume accounting explain missing space?"
                            .into(),
                        requires_approval: false,
                    });
                }
            }
            InvestigationFamily::FilesystemActivity => {}
        }
        let mut seen = std::collections::HashSet::new();
        checks.retain(|check| seen.insert(check.id.clone()));
        checks
    }

    pub fn validate_decision(
        &self,
        decision: &AgentDecision,
        available: &[AvailableCheck],
    ) -> Result<(), String> {
        let evidence = self
            .evidence
            .iter()
            .map(|item| item.id.as_str())
            .collect::<std::collections::HashSet<_>>();
        let hypotheses = self
            .hypotheses
            .iter()
            .map(|item| item.id.as_str())
            .collect::<std::collections::HashSet<_>>();
        let valid_evidence = |ids: &[String]| ids.iter().all(|id| evidence.contains(id.as_str()));
        match decision {
            AgentDecision::Check {
                check,
                evidence_ids,
                hypothesis_ids,
                reason,
            } => {
                if reason.trim().is_empty()
                    || !available.iter().any(|item| &item.id == check)
                    || !valid_evidence(evidence_ids)
                    || hypothesis_ids
                        .iter()
                        .any(|id| !hypotheses.contains(id.as_str()))
                {
                    return Err("AI returned an unsupported investigation decision.".into());
                }
            }
            AgentDecision::Finish {
                conclusion,
                evidence_ids,
                phase,
            } => {
                let supported = evidence_ids.iter().any(|id| {
                    self.evidence.iter().any(|evidence| {
                        &evidence.id == id
                            && evidence.status.can_support_hypothesis()
                            && (!evidence.supports.is_empty() || !evidence.contradicts.is_empty())
                    })
                });
                if conclusion.trim().is_empty()
                    || !valid_evidence(evidence_ids)
                    || (*phase == CasePhase::Complete && !supported)
                {
                    return Err("AI returned an unsupported investigation conclusion.".into());
                }
            }
            AgentDecision::Research { .. } | AgentDecision::AwaitApproval { .. } => {
                return Err("AI returned a legacy investigation decision.".into());
            }
        }
        Ok(())
    }

    fn supporting_evidence_ids(&self) -> Vec<String> {
        self.evidence
            .iter()
            .filter(|item| item.status.can_support_hypothesis())
            .map(|item| item.id.clone())
            .take(4)
            .collect()
    }

    pub fn apply_decision(&mut self, decision: &AgentDecision) {
        self.phase = match decision {
            AgentDecision::Check { .. } => CasePhase::Checking,
            AgentDecision::Research { .. } => CasePhase::Researching,
            AgentDecision::AwaitApproval { .. } => CasePhase::AwaitingApproval,
            AgentDecision::Finish {
                conclusion, phase, ..
            } => {
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
    status: EvidenceStatus,
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
        status,
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
            EvidenceStatus::Complete,
        ));
        assert_eq!(case.hypotheses[0].status, HypothesisStatus::Leading);
        assert!(case.conclusion.is_none());
    }

    #[test]
    fn failed_evidence_never_strengthens_a_hypothesis() {
        let mut case = InvestigationCase::new_fseventsd("fseventsd", 1, true);
        case.add_evidence(evidence(
            "fs-failed",
            EvidenceKind::FilesystemActivity,
            "fseventsd",
            "Authorization was not granted.",
            &["filesystem_activity"],
            &[],
            EvidenceStatus::PermissionRequired,
        ));
        assert_eq!(case.hypotheses[0].status, HypothesisStatus::Open);
        assert!(case.hypotheses[0].supporting_evidence.is_empty());
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

    #[test]
    fn storage_inventory_supports_distribution_without_calling_it_waste() {
        let case = InvestigationCase::new_storage("/example", 1, false);
        let mut observation =
            CheckObservation::complete(EvidenceKind::VolumeContext, "Measured direct children.");
        observation.supports.push("invented".into());
        case.classify_observation("inspect_children", &mut observation);
        assert_eq!(observation.supports, vec!["measured_children"]);
        assert!(!observation.supports.iter().any(|id| id.contains("waste")));
    }

    #[test]
    fn complete_conclusion_requires_related_usable_evidence() {
        let mut case = InvestigationCase::new_fseventsd("fseventsd", 1, false);
        case.add_evidence(evidence(
            "denied",
            EvidenceKind::FilesystemActivity,
            "fseventsd",
            "Authorization denied.",
            &[],
            &[],
            EvidenceStatus::PermissionRequired,
        ));
        let finish = AgentDecision::Finish {
            conclusion: "A cause was established.".into(),
            phase: CasePhase::Complete,
            evidence_ids: vec!["denied".into()],
        };
        assert!(case.validate_decision(&finish, &[]).is_err());
    }

    #[test]
    fn model_can_only_choose_an_available_check() {
        let case = InvestigationCase::new_storage("/example", 1, false);
        let decision = AgentDecision::Check {
            check: "run_shell".into(),
            reason: "Try an unrestricted command.".into(),
            evidence_ids: vec![],
            hypothesis_ids: vec![],
        };
        assert!(
            case.validate_decision(&decision, &case.available_checks(false))
                .is_err()
        );
    }
}
