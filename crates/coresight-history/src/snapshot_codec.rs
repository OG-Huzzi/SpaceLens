//! Strict persistence codecs for Phase 6.4 snapshot facts.
//!
//! Every enum that crosses the SQLite boundary encodes as its canonical
//! string form and decodes STRICTLY: an unknown persisted value is a
//! typed corruption detail, never a silent default. Encoding reuses the
//! types' own serde spellings (so the database can never drift from the
//! in-memory vocabulary), while decoding goes through serde as a strict
//! validator.
//!
//! Pure functions only: no database access, no I/O. The caller
//! ([`crate::snapshot`]) attaches table/column/run context and converts
//! details into [`crate::store::StoreError`].

use coresight_apps::{ApplicationSource, ProbedKind, SourceStatus};
use coresight_apps::{
    AssociationScope, CorrelationGroup, EvidenceKind, EvidenceSource, EvidenceStrength,
};
use coresight_apps::{Confidence as AppConfidence, FootprintKind, MatchedAttribute, PackageKind};
use coresight_capabilities::access::AccessState;
use coresight_classifier::{Category, Confidence as ClassifierConfidence, Subcategory};
use serde::de::DeserializeOwned;
use serde::Serialize;

/// Encode a simple (string-shaped) enum via its own serde spelling.
pub fn encode_enum<E: Serialize>(value: E) -> String {
    serde_json::to_value(&value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// Strictly decode a simple enum from its persisted string. Unknown
/// values fail — they are never defaulted.
pub fn decode_enum<E: DeserializeOwned>(persisted: &str) -> Result<E, String> {
    serde_json::from_str::<E>(&format!("\"{persisted}\""))
        .map_err(|_| format!("unknown value {persisted:?}"))
}

pub fn encode_application_source(v: ApplicationSource) -> String {
    encode_enum(v)
}

pub fn decode_application_source(persisted: &str) -> Result<ApplicationSource, String> {
    decode_enum(persisted)
}

pub fn encode_package_kind(v: PackageKind) -> String {
    encode_enum(v)
}

pub fn decode_package_kind(persisted: &str) -> Result<PackageKind, String> {
    decode_enum(persisted)
}

pub fn encode_source_status(v: SourceStatus) -> String {
    encode_enum(v)
}

pub fn decode_source_status(persisted: &str) -> Result<SourceStatus, String> {
    decode_enum(persisted)
}

pub fn encode_evidence_kind(v: EvidenceKind) -> String {
    encode_enum(v)
}

pub fn decode_evidence_kind(persisted: &str) -> Result<EvidenceKind, String> {
    decode_enum(persisted)
}

pub fn encode_evidence_source(v: EvidenceSource) -> String {
    encode_enum(v)
}

pub fn decode_evidence_source(persisted: &str) -> Result<EvidenceSource, String> {
    decode_enum(persisted)
}

pub fn encode_evidence_strength(v: EvidenceStrength) -> String {
    encode_enum(v)
}

pub fn decode_evidence_strength(persisted: &str) -> Result<EvidenceStrength, String> {
    decode_enum(persisted)
}

pub fn encode_scope(v: AssociationScope) -> String {
    encode_enum(v)
}

pub fn decode_scope(persisted: &str) -> Result<AssociationScope, String> {
    decode_enum(persisted)
}

pub fn encode_matched_attribute(v: MatchedAttribute) -> String {
    encode_enum(v)
}

pub fn decode_matched_attribute(persisted: &str) -> Result<MatchedAttribute, String> {
    decode_enum(persisted)
}

pub fn encode_app_confidence(v: AppConfidence) -> String {
    encode_enum(v)
}

pub fn decode_app_confidence(persisted: &str) -> Result<AppConfidence, String> {
    decode_enum(persisted)
}

pub fn encode_footprint_kind(v: FootprintKind) -> String {
    encode_enum(v)
}

pub fn decode_footprint_kind(persisted: &str) -> Result<FootprintKind, String> {
    decode_enum(persisted)
}

pub fn encode_probed_kind(v: ProbedKind) -> String {
    encode_enum(v)
}

pub fn decode_probed_kind(persisted: &str) -> Result<ProbedKind, String> {
    decode_enum(persisted)
}

pub fn encode_access(v: AccessState) -> String {
    encode_enum(v)
}

pub fn decode_access(persisted: &str) -> Result<AccessState, String> {
    decode_enum(persisted)
}

pub fn encode_category(v: Category) -> String {
    encode_enum(v)
}

pub fn decode_category(persisted: &str) -> Result<Category, String> {
    decode_enum(persisted)
}

pub fn encode_subcategory(v: Subcategory) -> String {
    encode_enum(v)
}

pub fn decode_subcategory(persisted: &str) -> Result<Subcategory, String> {
    decode_enum(persisted)
}

pub fn encode_classifier_confidence(v: ClassifierConfidence) -> String {
    encode_enum(v)
}

pub fn decode_classifier_confidence(persisted: &str) -> Result<ClassifierConfidence, String> {
    decode_enum(persisted)
}

/// The relationship-fact kinds the system model understands. These have no
/// serde form (plain engine enum), so the tags are explicit and match the
/// `relationship_obs` convention.
pub fn encode_relationship_fact_kind(
    v: coresight_system_model::RelationshipFactKind,
) -> &'static str {
    match v {
        coresight_system_model::RelationshipFactKind::ContentDuplicate => "CONTENT_DUPLICATE",
        coresight_system_model::RelationshipFactKind::HardLinkAlias => "HARD_LINK_ALIAS",
    }
}

pub fn decode_relationship_fact_kind(
    persisted: &str,
) -> Result<coresight_system_model::RelationshipFactKind, String> {
    match persisted {
        "CONTENT_DUPLICATE" => Ok(coresight_system_model::RelationshipFactKind::ContentDuplicate),
        "HARD_LINK_ALIAS" => Ok(coresight_system_model::RelationshipFactKind::HardLinkAlias),
        _ => Err(format!("unknown relationship fact kind {persisted:?}")),
    }
}

/// Encode a correlation group as (tag, inner-source) columns. Only
/// `SourceRecord` carries an inner source; every other group must store
/// NULL — a non-NULL inner on a singleton group is corruption, not data.
pub fn encode_correlation_group(group: &CorrelationGroup) -> (&'static str, Option<String>) {
    match group {
        CorrelationGroup::SourceRecord(inner) => ("SOURCE_RECORD", Some(encode_enum(inner))),
        CorrelationGroup::ObjectIdentity => ("OBJECT_IDENTITY", None),
        CorrelationGroup::BundleIdentifier => ("BUNDLE_IDENTIFIER", None),
        CorrelationGroup::InstallRootStructure => ("INSTALL_ROOT_STRUCTURE", None),
        CorrelationGroup::NameDerived => ("NAME_DERIVED", None),
        CorrelationGroup::ContentIdentity => ("CONTENT_IDENTITY", None),
        CorrelationGroup::ClassificationDerived => ("CLASSIFICATION_DERIVED", None),
        CorrelationGroup::HistoricalObservation => ("HISTORICAL_OBSERVATION", None),
    }
}

/// Strict group decode: unknown tags fail, a `SourceRecord` without a
/// decodable inner fails, and an unexpected inner on a singleton group
/// fails.
pub fn decode_correlation_group(
    tag: &str,
    inner: Option<&str>,
) -> Result<CorrelationGroup, String> {
    match tag {
        "SOURCE_RECORD" => match inner {
            Some(raw) => Ok(CorrelationGroup::SourceRecord(decode_enum::<
                ApplicationSource,
            >(raw)?)),
            None => Err("SOURCE_RECORD group without an inner source".to_string()),
        },
        "OBJECT_IDENTITY" => singleton(tag, inner, CorrelationGroup::ObjectIdentity),
        "BUNDLE_IDENTIFIER" => singleton(tag, inner, CorrelationGroup::BundleIdentifier),
        "INSTALL_ROOT_STRUCTURE" => singleton(tag, inner, CorrelationGroup::InstallRootStructure),
        "NAME_DERIVED" => singleton(tag, inner, CorrelationGroup::NameDerived),
        "CONTENT_IDENTITY" => singleton(tag, inner, CorrelationGroup::ContentIdentity),
        "CLASSIFICATION_DERIVED" => singleton(tag, inner, CorrelationGroup::ClassificationDerived),
        "HISTORICAL_OBSERVATION" => singleton(tag, inner, CorrelationGroup::HistoricalObservation),
        _ => Err(format!("unknown correlation group {tag:?}")),
    }
}

fn singleton(
    tag: &str,
    inner: Option<&str>,
    group: CorrelationGroup,
) -> Result<CorrelationGroup, String> {
    match inner {
        None => Ok(group),
        Some(_) => Err(format!(
            "correlation group {tag:?} must not carry an inner source"
        )),
    }
}

/// Verify a persisted application id against the ONE identity definition:
/// `normalized(name, publisher)`. A mismatch is corruption (tampered id,
/// tampered name/publisher, or a second identity scheme) — never a
/// silently accepted second identity.
pub fn verify_application_id(
    persisted_id: &str,
    name: &str,
    publisher: Option<&str>,
) -> Result<(), String> {
    if persisted_id.is_empty() {
        return Err("application with an empty id".to_string());
    }
    if name.is_empty() {
        return Err("application with an empty name".to_string());
    }
    let recomputed = coresight_apps::ApplicationId::derive(name, publisher);
    if recomputed.0 != persisted_id {
        return Err("application id does not match normalized (name, publisher)".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_simple_enum_round_trips() {
        assert_eq!(
            decode_application_source(&encode_application_source(
                ApplicationSource::RegistryUninstall
            ))
            .unwrap(),
            ApplicationSource::RegistryUninstall
        );
        for v in [
            PackageKind::Installed,
            PackageKind::Portable,
            PackageKind::SystemComponent,
            PackageKind::SharedRuntime,
            PackageKind::DependentComponent,
            PackageKind::Unknown,
        ] {
            assert_eq!(decode_package_kind(&encode_package_kind(v)).unwrap(), v);
        }
        for v in [
            SourceStatus::Complete,
            SourceStatus::Partial,
            SourceStatus::Unsupported,
            SourceStatus::Failed,
            SourceStatus::Unavailable,
        ] {
            assert_eq!(decode_source_status(&encode_source_status(v)).unwrap(), v);
        }
        for v in [
            EvidenceStrength::Weak,
            EvidenceStrength::Moderate,
            EvidenceStrength::Strong,
            EvidenceStrength::Direct,
        ] {
            assert_eq!(
                decode_evidence_strength(&encode_evidence_strength(v)).unwrap(),
                v
            );
        }
        for v in [
            AssociationScope::ThisMachine,
            AssociationScope::CurrentUser,
            AssociationScope::Unknown,
        ] {
            assert_eq!(decode_scope(&encode_scope(v)).unwrap(), v);
        }
        for v in [
            AppConfidence::Confirmed,
            AppConfidence::Strong,
            AppConfidence::Probable,
            AppConfidence::Possible,
            AppConfidence::Unknown,
        ] {
            assert_eq!(decode_app_confidence(&encode_app_confidence(v)).unwrap(), v);
        }
        for v in [
            AccessState::ReadSucceeded,
            AccessState::Empty,
            AccessState::DoesNotExist,
            AccessState::ExistsButInaccessible,
            AccessState::NotApplicable,
            AccessState::Unsupported,
            AccessState::Failed,
        ] {
            assert_eq!(decode_access(&encode_access(v)).unwrap(), v);
        }
        for v in [
            ClassifierConfidence::Unknown,
            ClassifierConfidence::Low,
            ClassifierConfidence::Medium,
            ClassifierConfidence::High,
        ] {
            assert_eq!(
                decode_classifier_confidence(&encode_classifier_confidence(v)).unwrap(),
                v
            );
        }
        for v in Category::ALL {
            assert_eq!(decode_category(&encode_category(v)).unwrap(), v);
        }
        for v in [
            ProbedKind::File,
            ProbedKind::Dir,
            ProbedKind::Symlink,
            ProbedKind::Other,
        ] {
            assert_eq!(decode_probed_kind(&encode_probed_kind(v)).unwrap(), v);
        }
    }

    #[test]
    fn unknown_enum_values_fail_loudly() {
        assert!(decode_application_source("SOMETHING_NEW").is_err());
        assert!(decode_package_kind("SOMETHING_NEW").is_err());
        assert!(decode_source_status("SOMETHING_NEW").is_err());
        assert!(decode_evidence_kind("SOMETHING_NEW").is_err());
        assert!(decode_evidence_source("SOMETHING_NEW").is_err());
        assert!(decode_evidence_strength("IMPOSSIBLE").is_err());
        assert!(decode_scope("SOMETHING_NEW").is_err());
        assert!(decode_matched_attribute("SOMETHING_NEW").is_err());
        assert!(decode_app_confidence("SOMETHING_NEW").is_err());
        assert!(decode_footprint_kind("SOMETHING_NEW").is_err());
        assert!(decode_probed_kind("SOMETHING_NEW").is_err());
        assert!(decode_access("SOMETHING_NEW").is_err());
        assert!(decode_category("SOMETHING_NEW").is_err());
        assert!(decode_subcategory("SOMETHING_NEW").is_err());
        assert!(decode_classifier_confidence("SOMETHING_NEW").is_err());
        assert!(decode_relationship_fact_kind("SOMETHING_NEW").is_err());
        assert!(decode_correlation_group("SOMETHING_NEW", None).is_err());
        // Empty strings are never valid enum values.
        assert!(decode_source_status("").is_err());
        assert!(decode_evidence_strength("").is_err());
    }

    #[test]
    fn correlation_groups_round_trip_with_strict_singletons() {
        let groups = [
            CorrelationGroup::SourceRecord(ApplicationSource::DesktopEntry),
            CorrelationGroup::ObjectIdentity,
            CorrelationGroup::BundleIdentifier,
            CorrelationGroup::InstallRootStructure,
            CorrelationGroup::NameDerived,
            CorrelationGroup::ContentIdentity,
            CorrelationGroup::ClassificationDerived,
            CorrelationGroup::HistoricalObservation,
        ];
        for g in groups {
            let (tag, inner) = encode_correlation_group(&g);
            let back = decode_correlation_group(tag, inner.as_deref()).unwrap();
            assert_eq!(back, g);
        }
        // A singleton carrying an inner source is corruption.
        assert!(decode_correlation_group("NAME_DERIVED", Some("X")).is_err());
        // A SourceRecord without its inner source is corruption.
        assert!(decode_correlation_group("SOURCE_RECORD", None).is_err());
        // A SourceRecord with an unknown inner source is corruption.
        assert!(decode_correlation_group("SOURCE_RECORD", Some("NOPE")).is_err());
    }

    #[test]
    fn application_identity_verification_follows_the_one_definition() {
        let id = coresight_apps::ApplicationId::derive("Foo App", Some("Acme"));
        assert!(verify_application_id(&id.0, "Foo App", Some("Acme")).is_ok());
        // Case/whitespace normalization still verifies (same definition).
        assert!(verify_application_id(&id.0, "  foo app ", Some("acme")).is_ok());
        // A changed name or publisher alters the id: mismatch fails.
        assert!(verify_application_id(&id.0, "Bar App", Some("Acme")).is_err());
        assert!(verify_application_id(&id.0, "Foo App", Some("Other")).is_err());
        assert!(verify_application_id(&id.0, "Foo App", None).is_err());
        // A changed source never alters the id: verification ignores it by
        // construction (it takes no source parameter).
        // Empty id or empty name fail loudly (never invented).
        assert!(verify_application_id("", "Foo App", Some("Acme")).is_err());
        assert!(verify_application_id(&id.0, "", Some("Acme")).is_err());
        assert!(verify_application_id("app-deadbeef", "Foo App", Some("Acme")).is_err());
    }
}
