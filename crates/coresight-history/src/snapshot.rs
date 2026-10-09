//! Phase 6.4: durable application-intelligence + system-model snapshots.
//!
//! ## Persistence boundary (explicit before any implementation)
//!
//! ```text
//! Existing persistent facts (Phases 1–5.1, untouched):
//!   scan_runs rows, observations rows, relationship_obs/_members rows,
//!   ConfigFingerprint, QueryLimits/retention, integrity_check +
//!   SchemaTooNew + Corrupt semantics, forward-only migrations v1–v4.
//!
//! New persistent facts (Phase 6.4, app_snapshot_* tables, v5):
//!   SystemModelInput verbatim — artifact facts (path/kind/identity/
//!   digest/size/access/classification), application facts (the full
//!   ApplicationRecord incl. id/name/publisher/paths/provenance/views,
//!   install roots, executable candidate, per-(app, artifact) ownership
//!   evidence), relationship facts (kind/object/digest/members),
//!   history facts (run/path/identity/category), source coverage rows,
//!   footprint candidates + footprint evidence, inventory/footprint
//!   truncation counters. Every row carries its run_id: snapshots are
//!   per-run history, never globally mutable "current" state.
//!
//! Facts that remain DERIVED (rebuilt, never stored):
//!   SystemModel nodes/edges/indexes, insights, candidates, claim
//!   assessments, application resolution states, ownership verdicts,
//!   query results, adjacency maps, authorization state.
//!
//! Facts that remain EPHEMERAL (never persisted):
//!   UI state, process handles, in-flight jobs, scan progress,
//!   executor state (none exists), network/cloud/licensing (none exist).
//!
//! Snapshot → construction mapping:
//!   database rows → strictly validated domain facts (lossless paths,
//!   full-width identity, ceiling-clamped evidence, id-verified records)
//!   → SystemModelInput → the SAME build_system_model() + finalize() +
//!   check_invariants() path as a fresh build. There is no second
//!   construction route: this module returns INPUTS, never a model.
//!
//! Migration version: v5 (forward-only, transactional, same convention
//! as v2–v4).
//! ```
//!
//! ## Invariants
//!
//! - Canonical facts only; no derived indexes/edges/insights/candidates.
//! - Lossless paths via the existing tagged encoding; full-width
//!   `ObjectIdentity` (device/inode/high as i64 bit-patterns).
//! - Evidence re-clamped through `OwnershipEvidence::new` (a tampered
//!   over-claimed strength is clamped, exactly as in-memory transport is).
//! - Application ids re-verified against `normalized(name, publisher)`.
//! - Every decode failure is `StoreError::Corrupt` — never a default.
//! - Commit is one transaction: re-commit of the same run replaces its
//!   snapshot atomically (idempotent, no semantic duplicates, no arrival
//!   order: facts are canonically sorted before ordinal assignment).
//! - Loads are per-run (`WHERE run_id = ?`), canonically ordered, and
//!   bounded by [`QueryLimits`]; unrelated runs are never materialized.
//! - `coresight-system-model` stays database-independent: this crate owns
//!   the SQL; the model crate only receives its plain input structs.

use std::path::PathBuf;

use coresight_apps::{
    ApplicationId, ApplicationRecord, ApplicationSource, FootprintCandidate, FootprintEvidence,
    FootprintReport, Inventory, OwnershipEvidence, PackageKind, SourceCoverage,
};
use coresight_identity::ObjectIdentity;
use coresight_system_model::{
    ApplicationFact, ArtifactClassification, ArtifactFact, HistoryFact, RelationshipFact,
    RelationshipFactKind, SystemModelInput,
};
use rusqlite::{params, OptionalExtension};

use crate::model::RunId;
use crate::store::{HistoryStore, QueryLimits, StoreError};

/// One application's persisted snapshot contribution: the record the
/// builder joins, plus the footprint candidates the footprint layer
/// published for it (with their evidence and the report counters).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppSnapshotFact {
    pub record: ApplicationRecord,
    pub install_roots: Vec<PathBuf>,
    pub executable: Option<PathBuf>,
    pub associations: Vec<(PathBuf, OwnershipEvidence)>,
    pub footprints: Vec<FootprintCandidate>,
}

/// Everything needed to rebuild one run's `SystemModelInput` from the
/// store: the input itself plus the inventory/footprint truncation
/// counters (which are honest incompleteness facts, not derived state).
///
/// `PartialEq` is deliberately NOT derived for the embedded
/// `SystemModelInput`: the builder's input carries no `PartialEq`, and
/// equality of a reloaded snapshot is proven against the *model* (which
/// does), never against the raw input vectors. Comparing reloaded facts
/// is done section-by-section where it matters.
#[derive(Debug, Clone, Default)]
pub struct SystemSnapshotInput {
    pub input: SystemModelInput,
    pub app_facts: Vec<AppSnapshotFact>,
    pub records_truncated: u64,
    pub records_rejected: u64,
    pub footprint: FootprintReport,
    /// Section names whose load hit the caller's `QueryLimits` cap. A
    /// NON-EMPTY list means the returned facts are a bounded PREFIX, not
    /// the whole snapshot — so the input must never be mistaken for
    /// complete, and [`HistoryStore::rebuild_system_model`] refuses it.
    pub load_truncated_sections: Vec<&'static str>,
}

impl SystemSnapshotInput {
    /// True when the caller's bound cut the fact set short. Bounded
    /// knowledge is never complete knowledge: a caller that sees this
    /// must raise its limit or report incompleteness.
    pub fn is_load_truncated(&self) -> bool {
        !self.load_truncated_sections.is_empty()
    }
}

/// The per-run snapshot presence: `None` = no snapshot was committed
/// for this run (absence, never an empty snapshot); `Some` = the row
/// counts of the committed snapshot (bounded listing support).
///
/// `RunId` wraps a `String` (not `Copy`), so `Copy` is not derivable;
/// the type stays cheap to clone and compares by value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotSummary {
    pub run_id: RunId,
    pub artifacts: u64,
    pub applications: u64,
    pub relationships: u64,
    pub history_facts: u64,
}

impl HistoryStore {
    /// Commit (or atomically re-commit) one run's system snapshot: the
    /// canonical `SystemModelInput` facts plus footprint candidates and
    /// truncation counters. One transaction; deterministic ordinal
    /// assignment after canonical sorting (insertion order is never
    /// semantic). The run row must exist (begin_run first); re-commit
    /// deletes the run's prior snapshot rows inside the same
    /// transaction, so retrying a commit can never create semantic
    /// duplicates.
    pub fn commit_system_snapshot(
        &mut self,
        run_id: &RunId,
        input: &SystemModelInput,
        app_facts: &[AppSnapshotFact],
        inventory: &Inventory,
        footprint: &FootprintReport,
    ) -> Result<(), StoreError> {
        // Fail closed on internal inconsistency: the two vectors must be
        // PARALLEL — same length, and the same logical application at each
        // index. The model is built from `input` (which carries the
        // evidence the model consumes); `app_facts` carries the same
        // records plus the footprint candidates. Anything else would let a
        // footprint attach to the wrong application, so it is rejected
        // before the transaction opens.
        if input.applications.len() != app_facts.len()
            || input
                .applications
                .iter()
                .zip(app_facts.iter())
                .any(|(i, f)| i.record.id.0 != f.record.id.0)
        {
            return Err(StoreError::Corrupt {
                table: "app_snapshot_apps",
                column: "app_id",
                run_id: Some(run_id.0.clone()),
                detail: "application facts are not parallel to the committed model input"
                    .to_string(),
            });
        }
        let conn = &mut self.conn;
        let tx = conn.transaction()?;
        let exists: Option<String> = tx
            .query_row(
                "SELECT run_id FROM scan_runs WHERE run_id = ?1",
                params![run_id.0],
                |r| r.get(0),
            )
            .optional()?;
        if exists.is_none() {
            return Err(StoreError::UnknownRun(run_id.clone()));
        }
        // Idempotent re-commit: clear prior snapshot rows first (same
        // transaction — atomic, never half-cleared).
        for table in [
            "app_snapshot_footprint_evidence",
            "app_snapshot_footprints",
            "app_snapshot_history",
            "app_snapshot_rel_members",
            "app_snapshot_relationships",
            "app_snapshot_coverage",
            "app_snapshot_evidence",
            "app_snapshot_roots",
            "app_snapshot_views",
            "app_snapshot_provenance",
            "app_snapshot_apps",
            "app_snapshot_artifacts",
            "app_snapshot_meta",
        ] {
            tx.execute(
                &format!("DELETE FROM {table} WHERE run_id = ?1"),
                params![run_id.0],
            )?;
        }

        tx.execute(
            "INSERT INTO app_snapshot_meta
             (run_id, records_truncated, records_rejected, fp_candidates_truncated,
              fp_children_truncated, fp_apps_truncated, fp_evidence_truncated)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                run_id.0,
                inventory.records_truncated as i64,
                inventory.records_rejected as i64,
                footprint.candidates_truncated as i64,
                footprint.children_truncated as i64,
                footprint.apps_truncated as i64,
                footprint.evidence_truncated as i64,
            ],
        )?;

        // ---- Artifacts (canonical path-byte order → ordinals). ----
        let mut artifacts = input.artifacts.clone();
        artifacts.sort_by(|a, b| {
            path_bytes(&a.path)
                .cmp(path_bytes(&b.path))
                .then(a.kind.cmp(&b.kind))
                .then(a.size.cmp(&b.size))
                .then(identity_key(&a.identity).cmp(&identity_key(&b.identity)))
        });
        {
            let mut stmt = tx.prepare(
                "INSERT INTO app_snapshot_artifacts
                 (run_id, artifact_ord, path, kind, size, device, inode, file_id_hi,
                  content_sha256, access, category, subcategory, confidence)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            )?;
            for (ord, a) in artifacts.iter().enumerate() {
                stmt.execute(params![
                    run_id.0,
                    ord as i64,
                    crate::path_encoding::encode(&a.path),
                    crate::snapshot_codec::encode_probed_kind(a.kind),
                    a.size.map(|v| v as i64),
                    a.identity.map(|id| id.volume as i64),
                    a.identity.map(|id| id.file_id as i64),
                    a.identity.and_then(|id| id.file_id_hi.map(|hi| hi as i64)),
                    a.content_sha256,
                    crate::snapshot_codec::encode_access(a.access),
                    a.classification
                        .map(|c| crate::snapshot_codec::encode_category(c.category)),
                    a.classification
                        .and_then(|c| c.subcategory)
                        .map(crate::snapshot_codec::encode_subcategory),
                    a.classification.map(|c| {
                        crate::snapshot_codec::encode_classifier_confidence(c.confidence)
                    }),
                ])?;
            }
        }

        // ---- Applications + per-app children (canonical id order). ----
        let mut app_order: Vec<usize> = (0..app_facts.len()).collect();
        app_order.sort_by(|&i, &j| {
            app_facts[i]
                .record
                .id
                .0
                .cmp(&app_facts[j].record.id.0)
                .then(
                    canonical_record_key(&app_facts[i].record)
                        .cmp(&canonical_record_key(&app_facts[j].record)),
                )
        });
        // For each committed APPLICATION FACT (not per id): input
        // duplicates under one id are distinct facts (conflict
        // preservation — the builder merges commutatively, never by
        // arrival), so each fact persists under its own fact_ord.
        let mut app_stmt = tx.prepare(
            "INSERT INTO app_snapshot_apps
             (run_id, app_id, fact_ord, name, version, publisher, install_location,
              install_date, estimated_size, uninstall_string, quiet_uninstall_string,
              modify_path, install_source, source, kind, system_component,
              bundle_identifier, executable_path, executable_candidate)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                     ?15, ?16, ?17, ?18, ?19)",
        )?;
        let mut prov_stmt = tx.prepare(
            "INSERT INTO app_snapshot_provenance (run_id, app_id, fact_ord, prov_ord, source)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;
        let mut view_stmt = tx.prepare(
            "INSERT INTO app_snapshot_views (run_id, app_id, fact_ord, view_ord, view)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;
        let mut root_stmt = tx.prepare(
            "INSERT INTO app_snapshot_roots (run_id, app_id, fact_ord, root_ord, path)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;
        let mut ev_stmt = tx.prepare(
            "INSERT INTO app_snapshot_evidence
             (run_id, app_id, fact_ord, artifact_path, evidence_ord, kind, source,
              strength, group_tag, group_source, scope, observed_path,
              matched_attribute, matched_value, matched_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        )?;
        let mut fp_stmt = tx.prepare(
            "INSERT INTO app_snapshot_footprints
             (run_id, app_id, fact_ord, footprint_ord, path, kind, confidence)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        let mut fp_ev_stmt = tx.prepare(
            "INSERT INTO app_snapshot_footprint_evidence
             (run_id, app_id, fact_ord, footprint_ord, evidence_ord, kind, confidence,
              source, scope, why)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        )?;
        for (fact_ord, &fi) in app_order.iter().enumerate() {
            let fact = &app_facts[fi];
            let input_fact = &input.applications[fi];
            let r = &fact.record;
            app_stmt.execute(params![
                run_id.0,
                r.id.0,
                fact_ord as i64,
                r.name,
                r.version,
                r.publisher,
                r.install_location
                    .as_ref()
                    .map(|p| crate::path_encoding::encode(p)),
                r.install_date,
                r.estimated_size_bytes.map(|v| v as i64),
                r.uninstall_string,
                r.quiet_uninstall_string,
                r.modify_path,
                r.install_source,
                crate::snapshot_codec::encode_application_source(r.source.clone()),
                crate::snapshot_codec::encode_package_kind(r.kind),
                if r.system_component { 1i64 } else { 0i64 },
                r.bundle_identifier,
                r.executable_path
                    .as_ref()
                    .map(|p| crate::path_encoding::encode(p)),
                fact.executable
                    .as_ref()
                    .map(|p| crate::path_encoding::encode(p)),
            ])?;
            let mut prov = r.provenance.clone();
            prov.sort();
            prov.dedup();
            for (prov_ord, source) in prov.iter().enumerate() {
                prov_stmt.execute(params![
                    run_id.0,
                    r.id.0,
                    fact_ord as i64,
                    prov_ord as i64,
                    crate::snapshot_codec::encode_application_source(source.clone()),
                ])?;
            }
            let mut views = r.observed_in_views.clone();
            views.sort();
            views.dedup();
            for (view_ord, view) in views.iter().enumerate() {
                view_stmt.execute(params![
                    run_id.0,
                    r.id.0,
                    fact_ord as i64,
                    view_ord as i64,
                    view,
                ])?;
            }
            let mut roots = fact.install_roots.clone();
            roots.sort_by(|a, b| path_bytes(a).cmp(path_bytes(b)));
            roots.dedup();
            for (root_ord, root) in roots.iter().enumerate() {
                root_stmt.execute(params![
                    run_id.0,
                    r.id.0,
                    fact_ord as i64,
                    root_ord as i64,
                    crate::path_encoding::encode(root),
                ])?;
            }
            // Associations: canonical (artifact-path, evidence) order.
            let mut assocs = fact.associations.clone();
            assocs.sort_by(|a, b| path_bytes(&a.0).cmp(path_bytes(&b.0)).then(a.1.cmp(&b.1)));
            // Group ordinals per artifact path (the PK includes the path).
            let mut per_artifact_ord: std::collections::BTreeMap<Vec<u8>, i64> =
                std::collections::BTreeMap::new();
            for (artifact_path, evidence) in &assocs {
                let key = path_bytes(artifact_path).to_vec();
                let ord = per_artifact_ord.entry(key).or_insert(0);
                let (group_tag, group_source) =
                    crate::snapshot_codec::encode_correlation_group(&evidence.correlation_group);
                ev_stmt.execute(params![
                    run_id.0,
                    r.id.0,
                    fact_ord as i64,
                    crate::path_encoding::encode(artifact_path),
                    *ord,
                    crate::snapshot_codec::encode_evidence_kind(evidence.kind),
                    crate::snapshot_codec::encode_evidence_source(evidence.source),
                    crate::snapshot_codec::encode_evidence_strength(evidence.strength),
                    group_tag,
                    group_source,
                    crate::snapshot_codec::encode_scope(evidence.scope),
                    crate::path_encoding::encode(&evidence.observed_path),
                    crate::snapshot_codec::encode_matched_attribute(evidence.matched_attribute),
                    evidence.matched_value,
                    evidence
                        .matched_path
                        .as_ref()
                        .map(|p| { crate::path_encoding::encode(p) }),
                ])?;
                *ord += 1;
            }
            // Also persist the input's association view identity check:
            // input_fact.associations must equal fact.associations as
            // multisets (same commit call produced both).
            {
                let mut x: Vec<(&PathBuf, &OwnershipEvidence)> = input_fact
                    .associations
                    .iter()
                    .map(|(p, e)| (p, e))
                    .collect();
                let mut y: Vec<(&PathBuf, &OwnershipEvidence)> =
                    fact.associations.iter().map(|(p, e)| (p, e)).collect();
                x.sort_by(|a, b| path_bytes(a.0).cmp(path_bytes(b.0)).then(a.1.cmp(b.1)));
                y.sort_by(|a, b| path_bytes(a.0).cmp(path_bytes(b.0)).then(a.1.cmp(b.1)));
                if x != y {
                    return Err(StoreError::Corrupt {
                        table: "app_snapshot_evidence",
                        column: "artifact_path",
                        run_id: Some(run_id.0.clone()),
                        detail: "application facts do not match the committed model input"
                            .to_string(),
                    });
                }
            }
            // Footprints for this app (canonical order → ordinals).
            let mut footprints = fact.footprints.clone();
            footprints.sort_by(|a, b| {
                path_bytes(&a.path)
                    .cmp(path_bytes(&b.path))
                    .then(a.kind.cmp(&b.kind))
                    .then(a.confidence.cmp(&b.confidence))
            });
            for (fp_ord, fp) in footprints.iter().enumerate() {
                fp_stmt.execute(params![
                    run_id.0,
                    r.id.0,
                    fact_ord as i64,
                    fp_ord as i64,
                    crate::path_encoding::encode(&fp.path),
                    crate::snapshot_codec::encode_footprint_kind(fp.kind),
                    crate::snapshot_codec::encode_app_confidence(fp.confidence),
                ])?;
                let mut fp_ev = fp.evidence.clone();
                fp_ev.sort();
                fp_ev.dedup();
                for (ev_ord, e) in fp_ev.iter().enumerate() {
                    fp_ev_stmt.execute(params![
                        run_id.0,
                        r.id.0,
                        fact_ord as i64,
                        fp_ord as i64,
                        ev_ord as i64,
                        crate::snapshot_codec::encode_evidence_kind(e.kind),
                        crate::snapshot_codec::encode_app_confidence(e.confidence),
                        e.source,
                        crate::snapshot_codec::encode_scope(e.scope),
                        e.why,
                    ])?;
                }
            }
        }
        drop(app_stmt);
        drop(prov_stmt);
        drop(view_stmt);
        drop(root_stmt);
        drop(ev_stmt);
        drop(fp_stmt);
        drop(fp_ev_stmt);

        // ---- Source coverage (canonical order → ordinals; duplicates kept
        // verbatim — the builder treats coverage as a multiset). ----
        let mut coverage = input.source_coverage.clone();
        coverage.sort_by(|a, b| {
            a.source
                .cmp(&b.source)
                .then(a.status.cmp(&b.status))
                .then(a.note.cmp(&b.note))
        });
        {
            let mut stmt = tx.prepare(
                "INSERT INTO app_snapshot_coverage (run_id, coverage_ord, source, status, note)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for (ord, c) in coverage.iter().enumerate() {
                stmt.execute(params![
                    run_id.0,
                    ord as i64,
                    c.source,
                    crate::snapshot_codec::encode_source_status(c.status),
                    c.note,
                ])?;
            }
        }

        // ---- Relationships (canonical order → ordinals). ----
        let mut rels = input.relationships.clone();
        rels.sort_by(|a, b| {
            rel_kind_key(a.kind)
                .cmp(&rel_kind_key(b.kind))
                .then(identity_key(&a.object).cmp(&identity_key(&b.object)))
                .then(a.content_sha256.cmp(&b.content_sha256))
                .then(sorted_path_keys(&a.paths).cmp(&sorted_path_keys(&b.paths)))
        });
        {
            let mut rel_stmt = tx.prepare(
                "INSERT INTO app_snapshot_relationships
                 (run_id, rel_ord, kind, object_device, object_inode, object_hi, content_sha256)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            let mut mem_stmt = tx.prepare(
                "INSERT INTO app_snapshot_rel_members (run_id, rel_ord, member_ord, path)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            for (rel_ord, rel) in rels.iter().enumerate() {
                rel_stmt.execute(params![
                    run_id.0,
                    rel_ord as i64,
                    crate::snapshot_codec::encode_relationship_fact_kind(rel.kind),
                    rel.object.map(|o| o.volume as i64),
                    rel.object.map(|o| o.file_id as i64),
                    rel.object.and_then(|o| o.file_id_hi.map(|hi| hi as i64)),
                    rel.content_sha256,
                ])?;
                let mut members = rel.paths.clone();
                members.sort_by(|a, b| path_bytes(a).cmp(path_bytes(b)));
                // Duplicate member paths are kept verbatim under ordinals
                // (the builder dedups canonically on reload — same result).
                for (mem_ord, member) in members.iter().enumerate() {
                    mem_stmt.execute(params![
                        run_id.0,
                        rel_ord as i64,
                        mem_ord as i64,
                        crate::path_encoding::encode(member),
                    ])?;
                }
            }
        }

        // ---- History facts (canonical order → ordinals; conflicting
        // (run, path) rows share keys by design — ordinals keep both). ----
        let mut history = input.history.clone();
        history.sort_by(|a, b| {
            a.run_id
                .cmp(&b.run_id)
                .then(path_bytes(&a.path).cmp(path_bytes(&b.path)))
                .then(identity_key(&a.identity).cmp(&identity_key(&b.identity)))
                .then(a.category.cmp(&b.category))
        });
        {
            let mut stmt = tx.prepare(
                "INSERT INTO app_snapshot_history
                 (run_id, hist_ord, hist_run_id, path, device, inode, file_id_hi, category)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            for (ord, h) in history.iter().enumerate() {
                stmt.execute(params![
                    run_id.0,
                    ord as i64,
                    h.run_id,
                    crate::path_encoding::encode(&h.path),
                    h.identity.map(|id| id.volume as i64),
                    h.identity.map(|id| id.file_id as i64),
                    h.identity.and_then(|id| id.file_id_hi.map(|hi| hi as i64)),
                    h.category,
                ])?;
            }
        }

        tx.commit()?;
        Ok(())
    }

    /// True when a system snapshot was committed for `run_id`.
    pub fn has_system_snapshot(&self, run_id: &RunId) -> Result<bool, StoreError> {
        let row: Option<String> = self
            .conn
            .query_row(
                "SELECT run_id FROM app_snapshot_meta WHERE run_id = ?1",
                params![run_id.0],
                |r| r.get(0),
            )
            .optional()?;
        Ok(row.is_some())
    }

    /// Bounded listing of committed system snapshots, newest run first.
    /// Only per-run COUNTS are materialized (never other runs' facts).
    pub fn list_system_snapshots(
        &self,
        limits: &QueryLimits,
    ) -> Result<Vec<SnapshotSummary>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT m.run_id, r.started_at,
                    (SELECT COUNT(*) FROM app_snapshot_artifacts a WHERE a.run_id = m.run_id),
                    (SELECT COUNT(*) FROM app_snapshot_apps a WHERE a.run_id = m.run_id),
                    (SELECT COUNT(*) FROM app_snapshot_relationships a WHERE a.run_id = m.run_id),
                    (SELECT COUNT(*) FROM app_snapshot_history a WHERE a.run_id = m.run_id)
             FROM app_snapshot_meta m JOIN scan_runs r ON r.run_id = m.run_id
             ORDER BY r.started_at DESC, m.run_id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limits.max_results as i64], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, u64>(2)?,
                r.get::<_, u64>(3)?,
                r.get::<_, u64>(4)?,
                r.get::<_, u64>(5)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, artifacts, applications, relationships, history_facts) = row?;
            out.push(SnapshotSummary {
                run_id: RunId(id),
                artifacts,
                applications,
                relationships,
                history_facts,
            });
        }
        Ok(out)
    }

    /// Load one run's system snapshot facts: strictly validated domain
    /// facts ready for the SAME `build_system_model()` path. Returns
    /// `None` when no snapshot was committed for the run (absence, never
    /// an empty input). Every multi-row load is `WHERE run_id = ?1`
    /// (never whole-database) with an explicit canonical `ORDER BY`, and
    /// the fact count is capped by `limits.max_results` per section.
    pub fn load_system_snapshot(
        &self,
        run_id: &RunId,
        limits: &QueryLimits,
    ) -> Result<Option<SystemSnapshotInput>, StoreError> {
        let meta: Option<(i64, i64, i64, i64, i64, i64)> = self
            .conn
            .query_row(
                "SELECT records_truncated, records_rejected, fp_candidates_truncated,
                        fp_children_truncated, fp_apps_truncated, fp_evidence_truncated
                 FROM app_snapshot_meta WHERE run_id = ?1",
                params![run_id.0],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .optional()?;
        let Some((records_truncated, records_rejected, fp_cand, fp_children, fp_apps, fp_ev)) =
            meta
        else {
            return Ok(None);
        };
        let run_tag = Some(run_id.0.clone());
        let records_truncated = u64_nonneg(
            records_truncated,
            "app_snapshot_meta",
            "records_truncated",
            &run_tag,
        )?;
        let records_rejected = u64_nonneg(
            records_rejected,
            "app_snapshot_meta",
            "records_rejected",
            &run_tag,
        )?;
        let fp_candidates_truncated = u64_nonneg(
            fp_cand,
            "app_snapshot_meta",
            "fp_candidates_truncated",
            &run_tag,
        )?;
        let fp_children_truncated = u64_nonneg(
            fp_children,
            "app_snapshot_meta",
            "fp_children_truncated",
            &run_tag,
        )?;
        let fp_apps_truncated =
            u64_nonneg(fp_apps, "app_snapshot_meta", "fp_apps_truncated", &run_tag)?;
        let fp_evidence_truncated = u64_nonneg(
            fp_ev,
            "app_snapshot_meta",
            "fp_evidence_truncated",
            &run_tag,
        )?;

        // ---- Artifacts. ----
        let mut caps = LoadCaps::default();
        let artifacts = self.load_snapshot_artifacts(run_id, limits, &run_tag, &mut caps)?;
        // ---- Applications (+ children grouped by (app_id, fact_ord)). ----
        let (app_facts, applications) =
            self.load_snapshot_apps(run_id, limits, &run_tag, &mut caps)?;
        // ---- Coverage. ----
        let source_coverage = self.load_snapshot_coverage(run_id, limits, &run_tag, &mut caps)?;
        // ---- Relationships. ----
        let relationships =
            self.load_snapshot_relationships(run_id, limits, &run_tag, &mut caps)?;
        // ---- History. ----
        let history = self.load_snapshot_history(run_id, limits, &run_tag, &mut caps)?;

        let footprint_candidates = flatten_footprints(&app_facts);
        Ok(Some(SystemSnapshotInput {
            input: SystemModelInput {
                artifacts,
                applications,
                relationships,
                history,
                source_coverage,
            },
            app_facts,
            records_truncated,
            records_rejected,
            footprint: FootprintReport {
                candidates: footprint_candidates,
                candidates_truncated: fp_candidates_truncated,
                children_truncated: fp_children_truncated,
                apps_truncated: fp_apps_truncated,
                evidence_truncated: fp_evidence_truncated,
            },
            load_truncated_sections: caps.into_sections(),
        }))
    }

    /// Load one run's application records in bounded, canonical form
    /// (records only — no evidence/footprints). Used by callers that need
    /// the inventory without rebuilding a full model input.
    pub fn load_snapshot_applications(
        &self,
        run_id: &RunId,
        limits: &QueryLimits,
    ) -> Result<Option<Vec<ApplicationRecord>>, StoreError> {
        let Some(loaded) = self.load_system_snapshot(run_id, limits)? else {
            return Ok(None);
        };
        Ok(Some(
            loaded.app_facts.into_iter().map(|f| f.record).collect(),
        ))
    }

    /// Rebuild the SAME validated system model from a persisted snapshot:
    /// load strictly validated inputs, then run the ONE canonical
    /// builder + invariant check. No second construction route exists —
    /// indexes derive inside the builder exactly as for a fresh build.
    pub fn rebuild_system_model(
        &self,
        run_id: &RunId,
        limits: &QueryLimits,
        model_limits: &coresight_system_model::SystemModelLimits,
    ) -> Result<Option<coresight_system_model::SystemModel>, StoreError> {
        let Some(loaded) = self.load_system_snapshot(run_id, limits)? else {
            return Ok(None);
        };
        // Fail closed on a bounded load: a model built from a PREFIX of the
        // stored facts would understate what the run observed — it could
        // drop claimants, edges and history context and then present the
        // remainder as the whole truth. Incomplete knowledge must never be
        // silently upgraded to a complete-looking model, so the caller is
        // told to raise its limit instead.
        if loaded.is_load_truncated() {
            return Err(StoreError::SnapshotBounded {
                run_id: run_id.0.clone(),
                sections: loaded.load_truncated_sections,
                limit: limits.max_results,
            });
        }
        let model = coresight_system_model::build_system_model(&loaded.input, model_limits);
        model
            .check_invariants()
            .map_err(|detail| StoreError::Corrupt {
                table: "app_snapshot_*",
                column: "model",
                run_id: Some(run_id.0.clone()),
                detail: format!("rebuilt model failed invariants: {detail}"),
            })?;
        Ok(Some(model))
    }

    // ---- Per-section loaders (all per-run, ordered, bounded). ----

    fn load_snapshot_artifacts(
        &self,
        run_id: &RunId,
        limits: &QueryLimits,
        run_tag: &Option<String>,
        caps: &mut LoadCaps,
    ) -> Result<Vec<ArtifactFact>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT path, kind, size, device, inode, file_id_hi, content_sha256,
                    access, category, subcategory, confidence
             FROM app_snapshot_artifacts WHERE run_id = ?1
             ORDER BY artifact_ord LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![run_id.0, probe_limit(limits)], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<i64>>(2)?,
                r.get::<_, Option<i64>>(3)?,
                r.get::<_, Option<i64>>(4)?,
                r.get::<_, Option<i64>>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, Option<String>>(8)?,
                r.get::<_, Option<String>>(9)?,
                r.get::<_, Option<String>>(10)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (
                path_raw,
                kind_raw,
                size_raw,
                dev_raw,
                ino_raw,
                hi_raw,
                digest,
                access_raw,
                cat_raw,
                sub_raw,
                conf_raw,
            ) = row?;
            let path = decode_snap_path(&path_raw, "app_snapshot_artifacts", "path", run_tag)?;
            let kind = crate::snapshot_codec::decode_probed_kind(&kind_raw)
                .map_err(|detail| corrupt("app_snapshot_artifacts", "kind", run_tag, detail))?;
            let size = opt_u64(size_raw, "app_snapshot_artifacts", "size", run_tag)?;
            let identity =
                snap_identity(dev_raw, ino_raw, hi_raw, "app_snapshot_artifacts", run_tag)?;
            let access = crate::snapshot_codec::decode_access(&access_raw)
                .map_err(|detail| corrupt("app_snapshot_artifacts", "access", run_tag, detail))?;
            let classification = match (cat_raw, sub_raw, conf_raw) {
                (Some(cat), sub, Some(conf)) => {
                    let category =
                        crate::snapshot_codec::decode_category(&cat).map_err(|detail| {
                            corrupt("app_snapshot_artifacts", "category", run_tag, detail)
                        })?;
                    let subcategory = sub
                        .map(|s| {
                            crate::snapshot_codec::decode_subcategory(&s).map_err(|detail| {
                                corrupt("app_snapshot_artifacts", "subcategory", run_tag, detail)
                            })
                        })
                        .transpose()?;
                    let confidence = crate::snapshot_codec::decode_classifier_confidence(&conf)
                        .map_err(|detail| {
                            corrupt("app_snapshot_artifacts", "confidence", run_tag, detail)
                        })?;
                    Some(ArtifactClassification {
                        category,
                        subcategory,
                        confidence,
                    })
                }
                (None, None, None) => None,
                _ => {
                    return Err(corrupt(
                        "app_snapshot_artifacts",
                        "category/subcategory/confidence",
                        run_tag,
                        "partial classification (category and confidence must travel together)"
                            .to_string(),
                    ));
                }
            };
            out.push(ArtifactFact {
                path,
                kind,
                identity,
                content_sha256: digest,
                size,
                access,
                classification,
            });
        }
        Ok(enforce_cap("artifacts", limits.max_results, out, caps))
    }

    #[allow(clippy::type_complexity)]
    fn load_snapshot_apps(
        &self,
        run_id: &RunId,
        limits: &QueryLimits,
        run_tag: &Option<String>,
        caps: &mut LoadCaps,
    ) -> Result<(Vec<AppSnapshotFact>, Vec<ApplicationFact>), StoreError> {
        // App rows in canonical commit order (fact_ord), capped.
        let mut stmt = self.conn.prepare(
            "SELECT app_id, fact_ord, name, version, publisher, install_location,
                    install_date, estimated_size, uninstall_string, quiet_uninstall_string,
                    modify_path, install_source, source, kind, system_component,
                    bundle_identifier, executable_path, executable_candidate
             FROM app_snapshot_apps WHERE run_id = ?1
             ORDER BY fact_ord LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![run_id.0, probe_limit(limits)], |r| {
            Ok(AppRow {
                app_id: r.get(0)?,
                fact_ord: r.get(1)?,
                name: r.get(2)?,
                version: r.get(3)?,
                publisher: r.get(4)?,
                install_location: r.get(5)?,
                install_date: r.get(6)?,
                estimated_size: r.get(7)?,
                uninstall_string: r.get(8)?,
                quiet_uninstall_string: r.get(9)?,
                modify_path: r.get(10)?,
                install_source: r.get(11)?,
                source: r.get(12)?,
                kind: r.get(13)?,
                system_component: r.get(14)?,
                bundle_identifier: r.get(15)?,
                executable_path: r.get(16)?,
                executable_candidate: r.get(17)?,
            })
        })?;
        let mut app_rows: Vec<AppRow> = Vec::new();
        for row in rows {
            app_rows.push(row?);
        }
        drop(stmt);
        let mut app_facts = Vec::with_capacity(app_rows.len());
        let mut applications = Vec::with_capacity(app_rows.len());
        for row in &app_rows {
            let record = load_snapshot_record(row, run_tag)?;
            let fact_ord = row.fact_ord;
            let app_id = record.id.0.clone();
            // Provenance: strictly decoded enums, canonically ordered and
            // deduplicated (the same union rule the builder applies). A
            // value this build cannot decode is corruption, never a
            // silently dropped source.
            let mut provenance: Vec<ApplicationSource> = Vec::new();
            for raw in self.load_child_strings(
                ChildStrings {
                    table: "app_snapshot_provenance",
                    column: "source",
                    ord_column: "prov_ord",
                },
                run_id,
                &app_id,
                fact_ord,
                limits,
                run_tag,
            )? {
                provenance.push(
                    crate::snapshot_codec::decode_application_source(&raw).map_err(|detail| {
                        corrupt("app_snapshot_provenance", "source", run_tag, detail)
                    })?,
                );
            }
            provenance.sort();
            provenance.dedup();
            let mut record = record;
            record.provenance = provenance;
            let mut observed_in_views = self.load_child_strings(
                ChildStrings {
                    table: "app_snapshot_views",
                    column: "view",
                    ord_column: "view_ord",
                },
                run_id,
                &app_id,
                fact_ord,
                limits,
                run_tag,
            )?;
            observed_in_views.sort();
            observed_in_views.dedup();
            record.observed_in_views = observed_in_views;
            let mut install_roots = Vec::new();
            for raw in self.load_child_strings(
                ChildStrings {
                    table: "app_snapshot_roots",
                    column: "path",
                    ord_column: "root_ord",
                },
                run_id,
                &app_id,
                fact_ord,
                limits,
                run_tag,
            )? {
                install_roots.push(decode_snap_path(
                    &raw,
                    "app_snapshot_roots",
                    "path",
                    run_tag,
                )?);
            }
            install_roots.sort_by(|a, b| path_bytes(a).cmp(path_bytes(b)));
            install_roots.dedup();
            let executable = row
                .executable_candidate
                .as_deref()
                .map(|raw| {
                    decode_snap_path(raw, "app_snapshot_apps", "executable_candidate", run_tag)
                })
                .transpose()?;
            let associations =
                self.load_snapshot_evidence(run_id, &record.id.0, fact_ord, limits, run_tag, caps)?;
            let footprints = self.load_snapshot_footprints(
                run_id,
                &record.id.0,
                fact_ord,
                limits,
                run_tag,
                caps,
            )?;
            // Provenance union across duplicate records under one id is a
            // BUILDER rule (commutative merge); reloaded facts keep their
            // per-fact provenance verbatim — the builder reunites them.
            app_facts.push(AppSnapshotFact {
                record: record.clone(),
                install_roots: install_roots.clone(),
                executable: executable.clone(),
                associations: associations.clone(),
                footprints,
            });
            applications.push(ApplicationFact {
                record,
                install_roots,
                executable,
                associations,
            });
        }
        Ok((
            enforce_cap("applications", limits.max_results, app_facts, caps),
            enforce_cap("applications", limits.max_results, applications, caps),
        ))
    }

    /// Ordered child strings of one application fact, read from a
    /// normalized child table. Bounded by `limits`; ordering comes from
    /// the ordinal column (a row join key, never a semantic tie-breaker).
    fn load_child_strings(
        &self,
        src: ChildStrings,
        run_id: &RunId,
        app_id: &str,
        fact_ord: i64,
        limits: &QueryLimits,
        run_tag: &Option<String>,
    ) -> Result<Vec<String>, StoreError> {
        let ChildStrings {
            table,
            column,
            ord_column,
        } = src;
        let sql = format!(
            "SELECT {column} FROM {table}
             WHERE run_id = ?1 AND app_id = ?2 AND fact_ord = ?3
             ORDER BY {ord_column} LIMIT ?4"
        );
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| corrupt(table, column, run_tag, format!("query failed: {e}")))?;
        let rows = stmt
            .query_map(
                params![run_id.0, app_id, fact_ord, probe_limit(limits)],
                |r| r.get::<_, String>(0),
            )
            .map_err(|e| corrupt(table, column, run_tag, format!("query failed: {e}")))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(
                row.map_err(|e| {
                    corrupt(table, column, run_tag, format!("row decode failed: {e}"))
                })?,
            );
        }
        Ok(out)
    }

    fn load_snapshot_evidence(
        &self,
        run_id: &RunId,
        app_id: &str,
        fact_ord: i64,
        limits: &QueryLimits,
        run_tag: &Option<String>,
        caps: &mut LoadCaps,
    ) -> Result<Vec<(PathBuf, OwnershipEvidence)>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT artifact_path, evidence_ord, kind, source, strength, group_tag,
                    group_source, scope, observed_path, matched_attribute,
                    matched_value, matched_path
             FROM app_snapshot_evidence
             WHERE run_id = ?1 AND app_id = ?2 AND fact_ord = ?3
             ORDER BY artifact_path, evidence_ord LIMIT ?4",
        )?;
        struct EvRow {
            artifact_path: String,
            kind: String,
            source: String,
            strength: String,
            group_tag: String,
            group_source: Option<String>,
            scope: String,
            observed_path: String,
            matched_attribute: String,
            matched_value: Option<String>,
            matched_path: Option<String>,
        }
        let rows = stmt.query_map(
            params![run_id.0, app_id, fact_ord, probe_limit(limits)],
            |r| {
                // Column order must match the SELECT list exactly:
                // artifact_path, evidence_ord, kind, source, strength,
                // group_tag, group_source, scope, observed_path,
                // matched_attribute, matched_value, matched_path.
                // (`evidence_ord` is a row join key, deliberately not
                // decoded: ordering already comes from ORDER BY.)
                Ok(EvRow {
                    artifact_path: r.get(0)?,
                    kind: r.get(2)?,
                    source: r.get(3)?,
                    strength: r.get(4)?,
                    group_tag: r.get(5)?,
                    group_source: r.get(6)?,
                    scope: r.get(7)?,
                    observed_path: r.get(8)?,
                    matched_attribute: r.get(9)?,
                    matched_value: r.get(10)?,
                    matched_path: r.get(11)?,
                })
            },
        )?;
        let mut out = Vec::new();
        for row in rows {
            let row = row?;
            let artifact_path = decode_snap_path(
                &row.artifact_path,
                "app_snapshot_evidence",
                "artifact_path",
                run_tag,
            )?;
            let kind = crate::snapshot_codec::decode_evidence_kind(&row.kind)
                .map_err(|detail| corrupt("app_snapshot_evidence", "kind", run_tag, detail))?;
            let source = crate::snapshot_codec::decode_evidence_source(&row.source)
                .map_err(|detail| corrupt("app_snapshot_evidence", "source", run_tag, detail))?;
            let requested = crate::snapshot_codec::decode_evidence_strength(&row.strength)
                .map_err(|detail| corrupt("app_snapshot_evidence", "strength", run_tag, detail))?;
            let group = crate::snapshot_codec::decode_correlation_group(
                &row.group_tag,
                row.group_source.as_deref(),
            )
            .map_err(|detail| corrupt("app_snapshot_evidence", "group_tag", run_tag, detail))?;
            let scope = crate::snapshot_codec::decode_scope(&row.scope)
                .map_err(|detail| corrupt("app_snapshot_evidence", "scope", run_tag, detail))?;
            let observed_path = decode_snap_path(
                &row.observed_path,
                "app_snapshot_evidence",
                "observed_path",
                run_tag,
            )?;
            let matched_attribute = crate::snapshot_codec::decode_matched_attribute(
                &row.matched_attribute,
            )
            .map_err(|detail| {
                corrupt(
                    "app_snapshot_evidence",
                    "matched_attribute",
                    run_tag,
                    detail,
                )
            })?;
            let matched_path = row
                .matched_path
                .as_deref()
                .map(|raw| decode_snap_path(raw, "app_snapshot_evidence", "matched_path", run_tag))
                .transpose()?;
            // Re-clamp through the ONE construction path: a tampered
            // over-claimed strength is clamped to kind/group ceilings
            // exactly as in-memory transport is (never trusted).
            let mut evidence = OwnershipEvidence::new(
                kind,
                source,
                requested,
                group,
                scope,
                observed_path,
                matched_attribute,
                row.matched_value,
            );
            if let Some(path) = matched_path {
                evidence = evidence.with_matched_path(path);
            }
            out.push((artifact_path, evidence));
        }
        Ok(enforce_cap("evidence", limits.max_results, out, caps))
    }

    fn load_snapshot_footprints(
        &self,
        run_id: &RunId,
        app_id: &str,
        fact_ord: i64,
        limits: &QueryLimits,
        run_tag: &Option<String>,
        caps: &mut LoadCaps,
    ) -> Result<Vec<FootprintCandidate>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT footprint_ord, path, kind, confidence
             FROM app_snapshot_footprints
             WHERE run_id = ?1 AND app_id = ?2 AND fact_ord = ?3
             ORDER BY footprint_ord LIMIT ?4",
        )?;
        let rows = stmt.query_map(
            params![run_id.0, app_id, fact_ord, probe_limit(limits)],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            },
        )?;
        let mut ords: Vec<(i64, String, String, String)> = Vec::new();
        for row in rows {
            ords.push(row?);
        }
        drop(stmt);
        let mut out = Vec::with_capacity(ords.len());
        for (fp_ord, path_raw, kind_raw, conf_raw) in ords {
            let path = decode_snap_path(&path_raw, "app_snapshot_footprints", "path", run_tag)?;
            let kind = crate::snapshot_codec::decode_footprint_kind(&kind_raw)
                .map_err(|detail| corrupt("app_snapshot_footprints", "kind", run_tag, detail))?;
            let confidence =
                crate::snapshot_codec::decode_app_confidence(&conf_raw).map_err(|detail| {
                    corrupt("app_snapshot_footprints", "confidence", run_tag, detail)
                })?;
            let mut ev_stmt = self.conn.prepare(
                "SELECT kind, confidence, source, scope, why
                 FROM app_snapshot_footprint_evidence
                 WHERE run_id = ?1 AND app_id = ?2 AND fact_ord = ?3 AND footprint_ord = ?4
                 ORDER BY evidence_ord LIMIT ?5",
            )?;
            let ev_rows = ev_stmt.query_map(
                params![run_id.0, app_id, fact_ord, fp_ord, probe_limit(limits)],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                    ))
                },
            )?;
            let mut evidence = Vec::new();
            for ev_row in ev_rows {
                let (kind_raw, conf_raw, source, scope_raw, why) = ev_row?;
                let kind =
                    crate::snapshot_codec::decode_evidence_kind(&kind_raw).map_err(|detail| {
                        corrupt("app_snapshot_footprint_evidence", "kind", run_tag, detail)
                    })?;
                let confidence =
                    crate::snapshot_codec::decode_app_confidence(&conf_raw).map_err(|detail| {
                        corrupt(
                            "app_snapshot_footprint_evidence",
                            "confidence",
                            run_tag,
                            detail,
                        )
                    })?;
                let scope = crate::snapshot_codec::decode_scope(&scope_raw).map_err(|detail| {
                    corrupt("app_snapshot_footprint_evidence", "scope", run_tag, detail)
                })?;
                if why.is_empty() {
                    return Err(corrupt(
                        "app_snapshot_footprint_evidence",
                        "why",
                        run_tag,
                        "footprint evidence without a reason".to_string(),
                    ));
                }
                evidence.push(FootprintEvidence {
                    kind,
                    confidence,
                    source,
                    scope,
                    why,
                });
            }
            let evidence = enforce_cap("footprint_evidence", limits.max_results, evidence, caps);
            out.push(FootprintCandidate {
                path,
                app: ApplicationId(app_id.to_string()),
                kind,
                confidence,
                evidence,
            });
        }
        Ok(enforce_cap("footprints", limits.max_results, out, caps))
    }

    fn load_snapshot_coverage(
        &self,
        run_id: &RunId,
        limits: &QueryLimits,
        run_tag: &Option<String>,
        caps: &mut LoadCaps,
    ) -> Result<Vec<SourceCoverage>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT source, status, note FROM app_snapshot_coverage
             WHERE run_id = ?1 ORDER BY coverage_ord LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![run_id.0, probe_limit(limits)], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (source, status_raw, note) = row?;
            if source.is_empty() {
                return Err(corrupt(
                    "app_snapshot_coverage",
                    "source",
                    run_tag,
                    "coverage without a source".to_string(),
                ));
            }
            let status = crate::snapshot_codec::decode_source_status(&status_raw)
                .map_err(|detail| corrupt("app_snapshot_coverage", "status", run_tag, detail))?;
            out.push(SourceCoverage {
                source,
                status,
                note,
            });
        }
        Ok(enforce_cap(
            "source_coverage",
            limits.max_results,
            out,
            caps,
        ))
    }

    fn load_snapshot_relationships(
        &self,
        run_id: &RunId,
        limits: &QueryLimits,
        run_tag: &Option<String>,
        caps: &mut LoadCaps,
    ) -> Result<Vec<RelationshipFact>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT rel_ord, kind, object_device, object_inode, object_hi, content_sha256
             FROM app_snapshot_relationships WHERE run_id = ?1
             ORDER BY rel_ord LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![run_id.0, probe_limit(limits)], |r| {
            Ok(RelationshipRow {
                rel_ord: r.get(0)?,
                kind: r.get(1)?,
                device: r.get(2)?,
                inode: r.get(3)?,
                high: r.get(4)?,
                digest: r.get(5)?,
            })
        })?;
        let mut ords: Vec<RelationshipRow> = Vec::new();
        for row in rows {
            ords.push(row?);
        }
        drop(stmt);
        let mut out = Vec::with_capacity(ords.len());
        for RelationshipRow {
            rel_ord,
            kind: kind_raw,
            device: dev_raw,
            inode: ino_raw,
            high: hi_raw,
            digest,
        } in ords
        {
            let kind = crate::snapshot_codec::decode_relationship_fact_kind(&kind_raw)
                .map_err(|detail| corrupt("app_snapshot_relationships", "kind", run_tag, detail))?;
            let object = snap_identity(
                dev_raw,
                ino_raw,
                hi_raw,
                "app_snapshot_relationships",
                run_tag,
            )?;
            let mut mem_stmt = self.conn.prepare(
                "SELECT path FROM app_snapshot_rel_members
                 WHERE run_id = ?1 AND rel_ord = ?2
                 ORDER BY member_ord LIMIT ?3",
            )?;
            let mem_rows = mem_stmt
                .query_map(params![run_id.0, rel_ord, probe_limit(limits)], |r| {
                    r.get::<_, String>(0)
                })?;
            let mut paths = Vec::new();
            for mem_row in mem_rows {
                paths.push(decode_snap_path(
                    &mem_row?,
                    "app_snapshot_rel_members",
                    "path",
                    run_tag,
                )?);
            }
            let paths = enforce_cap("relationship_members", limits.max_results, paths, caps);
            out.push(RelationshipFact {
                kind,
                paths,
                object,
                content_sha256: digest,
            });
        }
        Ok(enforce_cap("relationships", limits.max_results, out, caps))
    }

    fn load_snapshot_history(
        &self,
        run_id: &RunId,
        limits: &QueryLimits,
        run_tag: &Option<String>,
        caps: &mut LoadCaps,
    ) -> Result<Vec<HistoryFact>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT hist_run_id, path, device, inode, file_id_hi, category
             FROM app_snapshot_history WHERE run_id = ?1
             ORDER BY hist_ord LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![run_id.0, probe_limit(limits)], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<i64>>(2)?,
                r.get::<_, Option<i64>>(3)?,
                r.get::<_, Option<i64>>(4)?,
                r.get::<_, Option<String>>(5)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (hist_run_id, path_raw, dev_raw, ino_raw, hi_raw, category) = row?;
            if hist_run_id.is_empty() {
                return Err(corrupt(
                    "app_snapshot_history",
                    "hist_run_id",
                    run_tag,
                    "history fact without a run".to_string(),
                ));
            }
            let path = decode_snap_path(&path_raw, "app_snapshot_history", "path", run_tag)?;
            let identity =
                snap_identity(dev_raw, ino_raw, hi_raw, "app_snapshot_history", run_tag)?;
            out.push(HistoryFact {
                run_id: hist_run_id,
                path,
                identity,
                category,
            });
        }
        Ok(enforce_cap("history", limits.max_results, out, caps))
    }
}

// ---- Small helpers (pure, no SQL). ----

/// One persisted application row: the strictly-validated canonical
/// fields plus the fact ordinal the loader needs for its children.
/// Rows arrive in commit order (`fact_ord`), which is a stable row join
/// key — never a semantic tie-breaker (the model builder sees facts as a
/// multiset and merges commutatively).
struct AppRow {
    app_id: String,
    fact_ord: i64,
    name: String,
    version: Option<String>,
    publisher: Option<String>,
    install_location: Option<String>,
    install_date: Option<String>,
    estimated_size: Option<i64>,
    uninstall_string: Option<String>,
    quiet_uninstall_string: Option<String>,
    modify_path: Option<String>,
    install_source: Option<String>,
    source: String,
    kind: String,
    system_component: i64,
    bundle_identifier: Option<String>,
    executable_path: Option<String>,
    executable_candidate: Option<String>,
}

/// Strictly decode one persisted app row into an `ApplicationRecord`.
/// Pure: no database access — every failure is a typed corruption
/// detail, never a defaulted value.
fn load_snapshot_record(
    row: &AppRow,
    run_tag: &Option<String>,
) -> Result<ApplicationRecord, StoreError> {
    let source = crate::snapshot_codec::decode_application_source(&row.source)
        .map_err(|detail| corrupt("app_snapshot_apps", "source", run_tag, detail))?;
    let kind = crate::snapshot_codec::decode_package_kind(&row.kind)
        .map_err(|detail| corrupt("app_snapshot_apps", "kind", run_tag, detail))?;
    // Mandatory identity components: empty id/name fail loudly.
    crate::snapshot_codec::verify_application_id(&row.app_id, &row.name, row.publisher.as_deref())
        .map_err(|detail| corrupt("app_snapshot_apps", "app_id", run_tag, detail))?;
    let system_component = match row.system_component {
        0 => false,
        1 => true,
        other => {
            return Err(corrupt(
                "app_snapshot_apps",
                "system_component",
                run_tag,
                format!("must be 0 or 1, found {other}"),
            ));
        }
    };
    let estimated_size_bytes = opt_u64(
        row.estimated_size,
        "app_snapshot_apps",
        "estimated_size",
        run_tag,
    )?;
    let install_location = row
        .install_location
        .as_deref()
        .map(|raw| decode_snap_path(raw, "app_snapshot_apps", "install_location", run_tag))
        .transpose()?;
    let executable_path = row
        .executable_path
        .as_deref()
        .map(|raw| decode_snap_path(raw, "app_snapshot_apps", "executable_path", run_tag))
        .transpose()?;
    Ok(ApplicationRecord {
        id: ApplicationId(row.app_id.clone()),
        name: row.name.clone(),
        version: row.version.clone(),
        publisher: row.publisher.clone(),
        install_location,
        install_date: row.install_date.clone(),
        estimated_size_bytes,
        uninstall_string: row.uninstall_string.clone(),
        quiet_uninstall_string: row.quiet_uninstall_string.clone(),
        modify_path: row.modify_path.clone(),
        install_source: row.install_source.clone(),
        source,
        kind,
        system_component,
        observed_in_views: Vec::new(),
        bundle_identifier: row.bundle_identifier.clone(),
        executable_path,
        provenance: Vec::new(),
    })
}

fn corrupt(
    table: &'static str,
    column: &'static str,
    run_id: &Option<String>,
    detail: String,
) -> StoreError {
    StoreError::Corrupt {
        table,
        column,
        run_id: run_id.clone(),
        detail,
    }
}

fn decode_snap_path(
    raw: &str,
    table: &'static str,
    column: &'static str,
    run_id: &Option<String>,
) -> Result<PathBuf, StoreError> {
    if raw.is_empty() {
        return Err(corrupt(
            table,
            column,
            run_id,
            "empty stored path".to_string(),
        ));
    }
    crate::path_encoding::decode(raw)
        .map_err(|e| corrupt(table, column, run_id, format!("malformed stored path: {e}")))?
        .ok_or_else(|| corrupt(table, column, run_id, "undecodable stored path".to_string()))
}

/// Full-width identity from nullable (device, inode, high) columns,
/// reconstructed as the canonical `coresight_identity::ObjectIdentity`
/// the system model carries. Partial presence (device without inode or
/// vice versa) is corruption — a half-identity proves nothing and must
/// not become a narrowed fact. High bits attach exactly (bit-pattern
/// preserving u64↔i64 cast), so narrow and wide identities with the
/// same low pair stay distinct.
fn snap_identity(
    device: Option<i64>,
    inode: Option<i64>,
    high: Option<i64>,
    table: &'static str,
    run_id: &Option<String>,
) -> Result<Option<ObjectIdentity>, StoreError> {
    match (device, inode) {
        (Some(d), Some(i)) => Ok(Some(ObjectIdentity {
            volume: d as u64,
            file_id: i as u64,
            file_id_hi: high.map(|hi| hi as u64),
        })),
        (None, None) => {
            if high.is_some() {
                return Err(corrupt(
                    table,
                    "file_id_hi",
                    run_id,
                    "high identity bits without the low pair".to_string(),
                ));
            }
            Ok(None)
        }
        _ => Err(corrupt(
            table,
            "device/inode",
            run_id,
            "partial object identity (device without inode or vice versa)".to_string(),
        )),
    }
}

/// Non-negative u64 column (SQLite stores everything signed; negative
/// counts/sizes are corruption, never clamped to zero).
fn u64_nonneg(
    value: i64,
    table: &'static str,
    column: &'static str,
    run_id: &Option<String>,
) -> Result<u64, StoreError> {
    if value < 0 {
        return Err(corrupt(
            table,
            column,
            run_id,
            format!("negative count {value}"),
        ));
    }
    Ok(value as u64)
}

fn opt_u64(
    value: Option<i64>,
    table: &'static str,
    column: &'static str,
    run_id: &Option<String>,
) -> Result<Option<u64>, StoreError> {
    match value {
        None => Ok(None),
        Some(v) => Ok(Some(u64_nonneg(v, table, column, run_id)?)),
    }
}

/// Which normalized child table to read: the table name, its value
/// column, and its ordinal column. Grouping the three static names keeps
/// the loader signatures small and makes the table/column pair that owns
/// any corruption impossible to mix up.
#[derive(Debug, Clone, Copy)]
struct ChildStrings {
    table: &'static str,
    column: &'static str,
    ord_column: &'static str,
}

/// Names of the snapshot sections whose load hit the caller's cap.
///
/// Bounded loading is a *fact about the read*, and it must never be
/// silent: a capped section means the returned facts are a prefix, and a
/// model built from a prefix would understate what the run observed. The
/// loader records every capped section here so callers (and
/// `rebuild_system_model`) can refuse to treat partial facts as complete.
#[derive(Debug, Default)]
struct LoadCaps(Vec<&'static str>);

impl LoadCaps {
    fn note(&mut self, section: &'static str) {
        if !self.0.contains(&section) {
            self.0.push(section);
        }
    }

    fn into_sections(self) -> Vec<&'static str> {
        self.0
    }
}

/// Detect (and record) a cap hit: rows are fetched with `LIMIT limit + 1`,
/// so `len > limit` proves the section was cut short — exact, not guessed.
fn enforce_cap<T>(
    section: &'static str,
    limit: usize,
    mut rows: Vec<T>,
    caps: &mut LoadCaps,
) -> Vec<T> {
    if rows.len() > limit {
        rows.truncate(limit);
        caps.note(section);
    }
    rows
}

/// The `LIMIT` value that makes a cap hit provable.
fn probe_limit(limits: &QueryLimits) -> i64 {
    limits.max_results.saturating_add(1) as i64
}

/// One persisted relationship fact row (the identity-engine proof, as
/// projected by the caller). `rel_ord` is the row join key for members.
struct RelationshipRow {
    rel_ord: i64,
    kind: String,
    device: Option<i64>,
    inode: Option<i64>,
    high: Option<i64>,
    digest: Option<String>,
}

fn path_bytes(path: &std::path::Path) -> &[u8] {
    path.as_os_str().as_encoded_bytes()
}

fn identity_key(identity: &Option<coresight_identity::ObjectIdentity>) -> (u8, u64, u64, u8, u64) {
    match identity {
        None => (0, 0, 0, 0, 0),
        Some(id) => (
            1,
            id.volume,
            id.file_id,
            if id.file_id_hi.is_some() { 1 } else { 0 },
            id.file_id_hi.unwrap_or(0),
        ),
    }
}

fn rel_kind_key(kind: RelationshipFactKind) -> u8 {
    match kind {
        RelationshipFactKind::ContentDuplicate => 0,
        RelationshipFactKind::HardLinkAlias => 1,
    }
}

fn sorted_path_keys(paths: &[PathBuf]) -> Vec<Vec<u8>> {
    let mut keys: Vec<Vec<u8>> = paths.iter().map(|p| path_bytes(p).to_vec()).collect();
    keys.sort();
    keys
}

/// Canonical content key of one application record (every persisted
/// field except the id, which sorts first separately). Two facts under
/// one id with different content keep a deterministic commit order.
///
/// A concrete struct (not `impl Ord`) so the ordering is fully explicit
/// and every field's own `Ord` participates — no opaque return type.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct RecordKey {
    name: String,
    publisher: Option<String>,
    version: Option<String>,
    install_location: Option<Vec<u8>>,
    install_date: Option<String>,
    estimated_size_bytes: Option<u64>,
    uninstall_string: Option<String>,
    quiet_uninstall_string: Option<String>,
    modify_path: Option<String>,
    install_source: Option<String>,
    source: ApplicationSource,
    kind: PackageKind,
    system_component: bool,
    bundle_identifier: Option<String>,
    executable_path: Option<Vec<u8>>,
}

fn canonical_record_key(record: &ApplicationRecord) -> RecordKey {
    RecordKey {
        name: record.name.clone(),
        publisher: record.publisher.clone(),
        version: record.version.clone(),
        install_location: record
            .install_location
            .as_ref()
            .map(|p| path_bytes(p).to_vec()),
        install_date: record.install_date.clone(),
        estimated_size_bytes: record.estimated_size_bytes,
        uninstall_string: record.uninstall_string.clone(),
        quiet_uninstall_string: record.quiet_uninstall_string.clone(),
        modify_path: record.modify_path.clone(),
        install_source: record.install_source.clone(),
        source: record.source.clone(),
        kind: record.kind,
        system_component: record.system_component,
        bundle_identifier: record.bundle_identifier.clone(),
        executable_path: record
            .executable_path
            .as_ref()
            .map(|p| path_bytes(p).to_vec()),
    }
}

fn flatten_footprints(app_facts: &[AppSnapshotFact]) -> Vec<FootprintCandidate> {
    let mut out: Vec<FootprintCandidate> = app_facts
        .iter()
        .flat_map(|f| f.footprints.iter().cloned())
        .collect();
    out.sort_by(|a, b| {
        path_bytes(&a.path)
            .cmp(path_bytes(&b.path))
            .then(a.app.0.cmp(&b.app.0))
            .then(a.kind.cmp(&b.kind))
    });
    out.dedup_by(|a, b| a.path == b.path && a.app == b.app && a.kind == b.kind);
    out
}
