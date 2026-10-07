//! Shared fixtures for the Phase 6.3 system-model tests.
//!
//! Every fixture is synthetic and portable: no test here touches a real
//! filesystem, spawns a process, or reads the machine.

#![allow(dead_code)]

use std::path::PathBuf;

use coresight_apps::{
    ApplicationId, ApplicationRecord, ApplicationSource, CorrelationGroup, EvidenceKind,
    EvidenceSource, EvidenceStrength, MatchedAttribute, OwnershipEvidence, PackageKind, ProbedKind,
};
use coresight_capabilities::access::AccessState;
use coresight_classifier::{Category, Confidence as ClassificationConfidence, Subcategory};
use coresight_identity::ObjectIdentity;
use coresight_system_model::{
    ApplicationFact, ArtifactClassification, ArtifactFact, HistoryFact, RelationshipFact,
    RelationshipFactKind, SourceStateSummary, SystemModelInput,
};

/// One application record with sane defaults.
pub fn app_record(
    name: &str,
    publisher: Option<&str>,
    source: ApplicationSource,
) -> ApplicationRecord {
    ApplicationRecord {
        id: ApplicationId::derive(name, publisher),
        name: name.to_string(),
        version: None,
        publisher: publisher.map(str::to_string),
        install_location: None,
        install_date: None,
        estimated_size_bytes: None,
        uninstall_string: None,
        quiet_uninstall_string: None,
        modify_path: None,
        install_source: None,
        source: source.clone(),
        kind: PackageKind::Installed,
        system_component: false,
        observed_in_views: Vec::new(),
        bundle_identifier: None,
        executable_path: None,
        provenance: vec![source],
    }
}

/// An observed artifact with a proven object identity.
pub fn artifact(path: &str, volume: u64, file_id: u64, hi: Option<u64>) -> ArtifactFact {
    ArtifactFact {
        path: PathBuf::from(path),
        kind: ProbedKind::File,
        identity: Some(ObjectIdentity {
            volume,
            file_id,
            file_id_hi: hi,
        }),
        content_sha256: None,
        size: Some(1024),
        access: AccessState::ReadSucceeded,
        classification: None,
    }
}

/// An observed artifact with NO provable object identity.
pub fn artifact_no_identity(path: &str) -> ArtifactFact {
    ArtifactFact {
        path: PathBuf::from(path),
        kind: ProbedKind::File,
        identity: None,
        content_sha256: None,
        size: Some(512),
        access: AccessState::ReadSucceeded,
        classification: None,
    }
}

/// A directory artifact.
pub fn dir(path: &str) -> ArtifactFact {
    ArtifactFact {
        path: PathBuf::from(path),
        kind: ProbedKind::Dir,
        identity: Some(ObjectIdentity::narrow(1, 7)),
        content_sha256: None,
        size: None,
        access: AccessState::ReadSucceeded,
        classification: None,
    }
}

/// An artifact whose access was denied (proven to exist, unreadable).
pub fn denied_artifact(path: &str) -> ArtifactFact {
    ArtifactFact {
        path: PathBuf::from(path),
        kind: ProbedKind::Dir,
        identity: None,
        content_sha256: None,
        size: None,
        access: AccessState::ExistsButInaccessible,
        classification: None,
    }
}

/// Classify an artifact with the given category.
pub fn with_category(mut a: ArtifactFact, category: Category) -> ArtifactFact {
    a.classification = Some(ArtifactClassification {
        category,
        subcategory: None::<Subcategory>,
        confidence: ClassificationConfidence::High,
    });
    a
}

/// An artifact carrying a verified content digest.
pub fn with_content(mut a: ArtifactFact, sha256: &str) -> ArtifactFact {
    a.content_sha256 = Some(sha256.to_string());
    a
}

/// Install-location evidence for (app, path).
pub fn install_evidence(
    app: &ApplicationRecord,
    path: &str,
    strength: EvidenceStrength,
    kind: EvidenceKind,
    group: CorrelationGroup,
) -> OwnershipEvidence {
    OwnershipEvidence::new(
        kind,
        EvidenceSource::for_application_source(&app.source),
        strength,
        group,
        coresight_apps::AssociationScope::ThisMachine,
        PathBuf::from(path),
        MatchedAttribute::InstallLocation,
        Some(app.name.clone()),
    )
    .with_matched_path(PathBuf::from(path))
}

/// An ApplicationFact claiming `paths` with strong install-location evidence.
pub fn app_fact(
    name: &str,
    publisher: Option<&str>,
    roots: &[&str],
    associations: &[&str],
) -> ApplicationFact {
    let record = app_record(name, publisher, ApplicationSource::RegistryUninstall);
    let mut evidence = Vec::new();
    for p in associations {
        evidence.push((
            PathBuf::from(p),
            install_evidence(
                &record,
                p,
                EvidenceStrength::Direct,
                EvidenceKind::InstallLocation,
                CorrelationGroup::SourceRecord(record.source.clone()),
            ),
        ));
    }
    ApplicationFact {
        record,
        install_roots: roots.iter().map(PathBuf::from).collect(),
        executable: None,
        associations: evidence,
    }
}

/// An ApplicationFact whose only association is weak (name-derived).
pub fn app_fact_weak(name: &str, associations: &[&str]) -> ApplicationFact {
    let record = app_record(name, None, ApplicationSource::FilesystemPresence);
    let mut evidence = Vec::new();
    for p in associations {
        evidence.push((
            PathBuf::from(p),
            install_evidence(
                &record,
                p,
                EvidenceStrength::Weak,
                EvidenceKind::FilenameSimilarity,
                CorrelationGroup::NameDerived,
            ),
        ));
    }
    ApplicationFact {
        record,
        install_roots: Vec::new(),
        executable: None,
        associations: evidence,
    }
}

/// A containerment-only association (structural evidence, never ownership).
pub fn app_fact_containment(name: &str, associations: &[&str]) -> ApplicationFact {
    let record = app_record(name, None, ApplicationSource::FilesystemPresence);
    let mut evidence = Vec::new();
    for p in associations {
        evidence.push((
            PathBuf::from(p),
            install_evidence(
                &record,
                p,
                EvidenceStrength::Moderate,
                EvidenceKind::InstallRootContainment,
                CorrelationGroup::InstallRootStructure,
            ),
        ));
    }
    ApplicationFact {
        record,
        install_roots: Vec::new(),
        executable: None,
        associations: evidence,
    }
}

/// A MODERATE (credible but not strong) association — the shape of a shared
/// runtime or a structurally corroborated claim.
pub fn app_fact_moderate(name: &str, associations: &[&str]) -> ApplicationFact {
    let record = app_record(name, None, ApplicationSource::FilesystemPresence);
    let mut evidence = Vec::new();
    for p in associations {
        evidence.push((
            PathBuf::from(p),
            install_evidence(
                &record,
                p,
                EvidenceStrength::Moderate,
                EvidenceKind::BundleStructure,
                CorrelationGroup::InstallRootStructure,
            ),
        ));
    }
    ApplicationFact {
        record,
        install_roots: Vec::new(),
        executable: None,
        associations: evidence,
    }
}

/// A content-duplicate relationship fact.
pub fn content_duplicate(paths: &[&str], sha256: &str) -> RelationshipFact {
    RelationshipFact {
        kind: RelationshipFactKind::ContentDuplicate,
        paths: paths.iter().map(PathBuf::from).collect(),
        object: None,
        content_sha256: Some(sha256.to_string()),
    }
}

/// A hard-link alias relationship fact.
pub fn hard_link_alias(paths: &[&str], object: ObjectIdentity) -> RelationshipFact {
    RelationshipFact {
        kind: RelationshipFactKind::HardLinkAlias,
        paths: paths.iter().map(PathBuf::from).collect(),
        object: Some(object),
        content_sha256: None,
    }
}

/// A history fact for one path.
pub fn history(run_id: &str, path: &str, identity: Option<ObjectIdentity>) -> HistoryFact {
    HistoryFact {
        run_id: run_id.to_string(),
        path: PathBuf::from(path),
        identity,
        category: Some("CACHE".to_string()),
    }
}

/// Complete coverage: all sources read.
pub fn complete_coverage() -> Vec<coresight_apps::SourceCoverage> {
    vec![coresight_apps::SourceCoverage::complete("win32-uninstall")]
}

/// An unsupported source (e.g. MSIX on this build).
pub fn unsupported_coverage() -> Vec<coresight_apps::SourceCoverage> {
    vec![coresight_apps::SourceCoverage::with_status(
        "msix-appx",
        coresight_apps::SourceStatus::Unsupported,
        Some("MSIX/AppX enumeration is not implemented".to_string()),
    )]
}

/// An unavailable source.
pub fn unavailable_coverage() -> Vec<coresight_apps::SourceCoverage> {
    vec![coresight_apps::SourceCoverage::with_status(
        "win32-uninstall",
        coresight_apps::SourceStatus::Unavailable,
        Some("none of the uninstall views exist".to_string()),
    )]
}

/// A failed source.
pub fn failed_coverage() -> Vec<coresight_apps::SourceCoverage> {
    vec![coresight_apps::SourceCoverage::with_status(
        "win32-uninstall",
        coresight_apps::SourceStatus::Failed,
        Some("registry read failed".to_string()),
    )]
}

/// An input with the given parts.
pub fn input(
    artifacts: Vec<ArtifactFact>,
    applications: Vec<ApplicationFact>,
    relationships: Vec<RelationshipFact>,
    history: Vec<HistoryFact>,
    coverage: Vec<coresight_apps::SourceCoverage>,
) -> SystemModelInput {
    SystemModelInput {
        artifacts,
        applications,
        relationships,
        history,
        source_coverage: coverage,
    }
}

/// A tiny `SourceStateSummary` helper for assertions.
pub fn source_state(source: &str, status: coresight_apps::SourceStatus) -> SourceStateSummary {
    SourceStateSummary {
        source: source.to_string(),
        status,
        note: None,
    }
}
