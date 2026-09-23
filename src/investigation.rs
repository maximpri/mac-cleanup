// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded, evidence-driven investigations.
//!
//! The on-device model may choose which read-only tool is useful next, but
//! this module owns the case record, evidence provenance, and hypothesis
//! status. A case can therefore explain an unresolved cause without turning a
//! plausible story into a claim of fact.

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

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

impl CasePhase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Observing => "Observing",
            Self::Checking => "Checking",
            Self::Researching => "Reading reference material",
            Self::AwaitingApproval => "Waiting for your approval",
            Self::Verifying => "Verifying evidence",
            Self::Complete => "Complete",
            Self::Inconclusive => "Inconclusive",
        }
    }

    pub fn finished(self) -> bool {
        matches!(self, Self::Complete | Self::Inconclusive)
    }
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
    /// A free-form question asked through the Ask box.
    Question,
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

impl HypothesisStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Leading => "leading",
            Self::Weakened => "weakened",
            Self::Supported => "supported",
            Self::Unresolved => "unresolved",
        }
    }
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

/// A collector result before it becomes a numbered evidence record.
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

/// One tool call in an investigation timeline, including rejected calls.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub tool: String,
    pub label: String,
    /// The evidence record produced, if the call ran.
    #[serde(default)]
    pub evidence_id: Option<String>,
    pub status: EvidenceStatus,
    #[serde(default)]
    pub elapsed_ms: u64,
    /// False when the measured fallback, not the model, chose the call.
    #[serde(default)]
    pub chosen_by_model: bool,
    /// Why the call did not run, if it was rejected.
    #[serde(default)]
    pub rejected: Option<String>,
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
    /// Tool calls used, including rejected ones.
    pub decision_count: u8,
    /// Tool calls allowed.
    pub decision_budget: u8,
    pub hypotheses: Vec<Hypothesis>,
    pub evidence: Vec<EvidenceRecord>,
    pub checks_run: Vec<String>,
    pub conclusion: Option<String>,
    /// The Ask-box question, for question cases.
    #[serde(default)]
    pub question: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<ToolCallRecord>,
    /// Validated plan-action IDs the model suggested. Never added automatically.
    #[serde(default)]
    pub suggested_actions: Vec<String>,
}

impl InvestigationCase {
    pub fn new_fseventsd(target: impl Into<String>, revision: u64, automatic: bool) -> Self {
        let mut case = Self::base(
            target,
            revision,
            automatic,
            InvestigationFamily::FilesystemActivity,
        );
        case.hypotheses = vec![
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
        ];
        case
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
                "Open handles, recent changes, and comparable history can establish activity without claiming ownership.",
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
                "Open-handle and recent-change evidence can prevent cleanup while work is active.",
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

    /// A question has no predefined hypotheses; its report must still cite evidence.
    pub fn new_question(question: impl Into<String>, revision: u64) -> Self {
        let question = question.into();
        let mut case = Self::base(
            question.clone(),
            revision,
            false,
            InvestigationFamily::Question,
        );
        case.question = Some(question);
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
            .unwrap_or_default();
        Self {
            id: format!("investigation:{family:?}:{}", now.as_nanos()),
            target: target.into(),
            family,
            started_at: now.as_secs(),
            revision,
            automatic,
            phase: CasePhase::Observing,
            decision_count: 0,
            decision_budget: 0,
            hypotheses: Vec::new(),
            evidence: Vec::new(),
            checks_run: Vec::new(),
            conclusion: None,
            question: None,
            tool_calls: Vec::new(),
            suggested_actions: Vec::new(),
        }
    }

    /// Record evidence. Only complete or partial evidence can change a
    /// hypothesis, and relationships to unknown hypotheses are dropped.
    pub fn add_evidence(&mut self, mut evidence: EvidenceRecord) {
        let known = |id: &String| self.hypotheses.iter().any(|h| &h.id == id);
        evidence.supports.retain(known);
        evidence.contradicts.retain(known);
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
        if !self.phase.finished() && self.phase != CasePhase::AwaitingApproval {
            self.phase = CasePhase::Checking;
        }
    }

    pub fn evidence_by_id(&self, id: &str) -> Option<&EvidenceRecord> {
        self.evidence.iter().find(|evidence| evidence.id == id)
    }

    /// Usable evidence that supports `hypothesis`, among the cited IDs.
    pub fn cited_support(&self, hypothesis: &str, cited: &[String]) -> bool {
        cited.iter().any(|id| {
            self.evidence_by_id(id).is_some_and(|evidence| {
                evidence.status.can_support_hypothesis()
                    && evidence
                        .supports
                        .iter()
                        .any(|supported| supported == hypothesis)
            })
        })
    }

    /// Any usable evidence that contradicts `hypothesis`.
    pub fn contradicted(&self, hypothesis: &str) -> bool {
        self.evidence.iter().any(|evidence| {
            evidence.status.can_support_hypothesis()
                && evidence.contradicts.iter().any(|id| id == hypothesis)
        })
    }

    /// A measured, model-free conclusion.
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
        if self.hypotheses.is_empty() {
            "Measured checks finished; the evidence list shows what was observed.".into()
        } else if supported.is_empty() {
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
    fn evidence_updates_hypothesis_status_without_claiming_causation() {
        let mut case = InvestigationCase::new_fseventsd("fseventsd", 1, true);
        case.add_evidence(evidence(
            "E1",
            EvidenceKind::FilesystemActivity,
            "fseventsd",
            "Repeated swap-backed filesystem reads observed.",
            &["filesystem_activity"],
            &[],
            EvidenceStatus::Complete,
        ));
        assert_eq!(case.hypotheses[0].status, HypothesisStatus::Leading);
        assert!(case.conclusion.is_none());
        assert!(case.cited_support("filesystem_activity", &["E1".into()]));
        assert!(!case.cited_support("filesystem_activity", &["E9".into()]));
    }

    #[test]
    fn failed_evidence_never_strengthens_a_hypothesis() {
        let mut case = InvestigationCase::new_fseventsd("fseventsd", 1, true);
        case.add_evidence(evidence(
            "E1",
            EvidenceKind::FilesystemActivity,
            "fseventsd",
            "Authorization was not granted.",
            &["filesystem_activity"],
            &[],
            EvidenceStatus::PermissionRequired,
        ));
        assert_eq!(case.hypotheses[0].status, HypothesisStatus::Open);
        assert!(case.hypotheses[0].supporting_evidence.is_empty());
        assert!(!case.cited_support("filesystem_activity", &["E1".into()]));
    }

    #[test]
    fn unknown_relationships_are_dropped_before_recording() {
        let mut case = InvestigationCase::new_storage("/example", 1, false);
        case.add_evidence(evidence(
            "E1",
            EvidenceKind::VolumeContext,
            "/example",
            "Measured direct children.",
            &["measured_children", "invented_waste"],
            &["process_persists"],
            EvidenceStatus::Complete,
        ));
        assert_eq!(case.evidence[0].supports, vec!["measured_children"]);
        assert!(case.evidence[0].contradicts.is_empty());
    }

    #[test]
    fn questions_have_no_hypotheses_and_older_records_still_load() {
        let case = InvestigationCase::new_question("Why is my disk full?", 3);
        assert_eq!(case.family, InvestigationFamily::Question);
        assert!(case.hypotheses.is_empty());
        assert_eq!(case.question.as_deref(), Some("Why is my disk full?"));
        let legacy = r#"{"id":"investigation:fseventsd:1","target":"fseventsd","family":"filesystem_activity","started_at":1,"revision":1,"automatic":true,"phase":"researching","decision_count":2,"decision_budget":3,"hypotheses":[],"evidence":[],"checks_run":["fs_usage"],"conclusion":null}"#;
        let parsed: InvestigationCase = serde_json::from_str(legacy).unwrap();
        assert!(parsed.tool_calls.is_empty() && parsed.question.is_none());
        assert_eq!(parsed.phase, CasePhase::Researching);
    }
}
