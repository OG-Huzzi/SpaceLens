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
//! - The application vectors a commit accepts must be PARALLEL and equal
//!   field-for-field, and the footprint report must describe exactly the
//!   facts being stored; a divergence is rejected before the transaction
//!   opens, so what is persisted always describes the model's input.
//! - Loads are per-run (`WHERE run_id = ?`), canonically ordered, and
//!   bounded by [`QueryLimits`]; unrelated runs are never materialized.
//!   A cap hit in ANY section is reported and refuses a rebuild.
//! - `coresight-system-model` stays database-independent: this crate owns
//!   the SQL; the model crate only receives its plain input structs.
//!
//! ## Normalization (what "round-trip" means per field)
//!
//! Values that are SETS in the domain — application `provenance`,
//! `observed_in_views`, and `install_roots` — are stored and reloaded
//! canonically ordered and deduplicated (the 6.2 producers already union
//! them, and the model reads them as sets). Everything else is stored
//! verbatim, including duplicate and conflicting facts: a repeated
//! relationship or history row keeps its own row under its own ordinal,
//! because the model's inputs are multisets and a later input must never
//! silently overwrite an earlier contradictory one.

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
use crate::snapshot_codec::verify_application_id;
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

/// One run's application records together with the sections the caller's
/// bound cut short — so a partial inventory is never mistaken for the
/// complete application set.
#[derive(Debug, Clone)]
pub struct LoadedApplications {
    pub records: Vec<ApplicationRecord>,
    pub load_truncated_sections: Vec<&'static str>,
}

impl LoadedApplications {
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
        // Fail closed on internal inconsistency. The two vectors must be
        // PARALLEL — same length, and the SAME fact at each index, field
        // for field. The model is built from `input`; `app_facts` is the
        // same data plus the footprint candidates, and only `app_facts`
        // is persisted. Comparing just the ids would let a caller whose
        // vectors disagree (different record content, install roots, or
        // executable) silently persist one copy while the model used the
        // other, so everything the snapshot stores is compared here,
        // before the transaction opens.
        if input.applications.len() != app_facts.len() {
            return Err(parallel_mismatch(run_id, "length"));
        }
        // Identity preflight (Phase 6.4.2, Workstream A): matching parallel
        // vectors only prove the two copies agree with EACH OTHER — they
        // prove nothing about the identity they carry. A caller can supply a
        // forged `ApplicationId` consistently in every input, the parallel
        // checks above all pass, and the row is written under a name that
        // does not produce it. That state would be un-persistable: the
        // loader's `verify_application_id` rejects it on read, so the commit
        // would look successful and then fail to reload. The id is
        // therefore checked against the ONE canonical rule BEFORE the
        // transaction opens, using the shared verifier.
        for f in app_facts {
            verify_application_id(
                &f.record.id.0,
                &f.record.name,
                f.record.publisher.as_deref(),
            )
            .map_err(|detail| StoreError::Corrupt {
                table: "app_snapshot_apps",
                column: "app_id",
                run_id: Some(run_id.0.clone()),
                detail,
            })?;
        }
        for (i, f) in input.applications.iter().zip(app_facts.iter()) {
            if i.record != f.record {
                return Err(parallel_mismatch(run_id, "record"));
            }
            if i.install_roots != f.install_roots {
                return Err(parallel_mismatch(run_id, "install_roots"));
            }
            if i.executable != f.executable {
                return Err(parallel_mismatch(run_id, "executable"));
            }
            if i.associations != f.associations {
                return Err(parallel_mismatch(run_id, "associations"));
            }
            // A footprint rides along its application fact, so its `app`
            // must BE that fact's application. A candidate attributed to
            // different software would silently relocate a scope onto the
            // wrong application on reload, so it is rejected here.
            for fp in &f.footprints {
                if fp.app != f.record.id {
                    return Err(StoreError::Corrupt {
                        table: "app_snapshot_footprints",
                        column: "footprint_ord",
                        run_id: Some(run_id.0.clone()),
                        detail: format!(
                            "footprint for {} is attributed to a different application ({})",
                            f.record.id.0, fp.app.0
                        ),
                    });
                }
            }
        }
        // The inventory's records must describe the same applications the
        // snapshot stores. `Inventory::records` is the DEDUPLICATED
        // merged discovery result, while the model input is a fact
        // MULTISET (two facts under one id are two records, not one), so
        // the comparison is over the SET of ids: every stored id must be
        // covered, and the inventory must not claim an application whose
        // facts were never stored. Otherwise the stored counters would
        // describe records that were never stored.
        {
            let from_inventory: std::collections::BTreeSet<&ApplicationId> =
                inventory.records.iter().map(|r| &r.id).collect();
            let from_facts: std::collections::BTreeSet<&ApplicationId> =
                app_facts.iter().map(|f| &f.record.id).collect();
            if from_inventory != from_facts {
                return Err(StoreError::Corrupt {
                    table: "app_snapshot_meta",
                    column: "records_truncated",
                    run_id: Some(run_id.0.clone()),
                    detail: "inventory records do not describe the committed application \
                             facts"
                        .to_string(),
                });
            }
        }
        // The footprint report's counters AND candidates must describe the
        // same facts that get persisted. Candidates are stored per
        // application (the normalized representation), so the report's
        // list must be exactly their canonical union — otherwise the
        // stored counters could describe candidates that were never
        // stored. Canonicalized before comparison: the union rule is
        // commutative, so a differently-ordered report is not an error.
        {
            let mut from_report = footprint.candidates.clone();
            canonicalize_footprints(&mut from_report);
            let mut from_facts = flatten_footprints(app_facts);
            canonicalize_footprints(&mut from_facts);
            if from_report != from_facts {
                return Err(StoreError::Corrupt {
                    table: "app_snapshot_footprints",
                    column: "footprint_ord",
                    run_id: Some(run_id.0.clone()),
                    detail: "footprint report candidates do not match the committed \
                             application facts"
                        .to_string(),
                });
            }
        }
        let run_tag = Some(run_id.0.clone());
        // Every numeric conversion happens BEFORE the snapshot transaction
        // opens, so a value the store cannot represent rejects the commit
        // without touching the existing snapshot at all (Workstream C).
        let records_truncated = checked_counter_i64(
            inventory.records_truncated,
            "app_snapshot_meta",
            "records_truncated",
            &run_tag,
        )?;
        let records_rejected = checked_counter_i64(
            inventory.records_rejected,
            "app_snapshot_meta",
            "records_rejected",
            &run_tag,
        )?;
        let fp_candidates_truncated = checked_counter_i64(
            footprint.candidates_truncated,
            "app_snapshot_meta",
            "fp_candidates_truncated",
            &run_tag,
        )?;
        let fp_children_truncated = checked_counter_i64(
            footprint.children_truncated,
            "app_snapshot_meta",
            "fp_children_truncated",
            &run_tag,
        )?;
        let fp_apps_truncated = checked_counter_i64(
            footprint.apps_truncated,
            "app_snapshot_meta",
            "fp_apps_truncated",
            &run_tag,
        )?;
        let fp_evidence_truncated = checked_counter_i64(
            footprint.evidence_truncated,
            "app_snapshot_meta",
            "fp_evidence_truncated",
            &run_tag,
        )?;

        // ---- TRUE preflight (Phase 6.4.6, Workstream B). ----
        //
        // The write path below converts artifact sizes inline and computes
        // every ordinal (artifact, fact, provenance, view, root, evidence,
        // footprint, footprint-evidence, coverage, relationship, member,
        // history) inside the loops that INSERT — AFTER the run's prior
        // snapshot rows have been DELETEd. A conversion that can fail
        // therefore used to be discovered only after destructive statements
        // had already run, relying on transaction rollback rather than
        // never touching existing data. The preflight below precomputes
        // EVERY conversion that the write path will need BEFORE the
        // transaction opens, into one frozen persistence plan. The write
        // path then binds these values verbatim: no conversion can fail
        // after the first mutation, because every conversion already ran.
        //
        // The plan is independent of the destructive statements and of
        // SQLite: it is built from the SAME canonically-sorted,
        // normalized collections the write path persists (frozen here so
        // the write loop cannot diverge from the preflight), and every
        // checked value is asserted against the plan at bind time — a
        // `debug_assert_eq!` that dies loudly in debug builds if the two
        // ever disagree, so the "verbatim" invariant cannot silently rot.
        //
        // Object identity is deliberately NOT converted: the device /
        // inode / high-bit columns are intentional bit-pattern storage
        // (`u64::MAX` round-trips exactly, by tested contract), so there is
        // no fallible conversion to preflight.
        let plan = preflight_snapshot_plan(input, app_facts, &run_tag)?;

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
                records_truncated,
                records_rejected,
                fp_candidates_truncated,
                fp_children_truncated,
                fp_apps_truncated,
                fp_evidence_truncated,
            ],
        )?;

        // ---- Artifacts (canonical path-byte order → ordinals). ----
        //
        // The collection is re-sorted with the SHARED sorter so the write
        // order is definitionally the preflight order; the frozen sizes
        // and the checked ordinal cursor are then bound verbatim.
        let mut artifacts = input.artifacts.clone();
        sort_artifacts(&mut artifacts);
        debug_assert_eq!(
            artifacts.len(),
            plan.artifact_len,
            "preflight/write artifact length diverged"
        );
        let artifact_ords = CheckedOrdinals {
            column: "artifact_ord",
            len: plan.artifact_len,
        };
        {
            let mut stmt = tx.prepare(
                "INSERT INTO app_snapshot_artifacts
                 (run_id, artifact_ord, path, kind, size, device, inode, file_id_hi,
                  content_sha256, access, category, subcategory, confidence)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            )?;
            for (ord, a) in artifacts.iter().enumerate() {
                let artifact_ord = artifact_ords.at(ord);
                // Preflighted verbatim: the size was already checked for
                // this exact position, so no conversion can fail here.
                let size = plan.artifact_sizes[ord];
                stmt.execute(params![
                    run_id.0,
                    artifact_ord,
                    crate::path_encoding::encode(&a.path),
                    crate::snapshot_codec::encode_probed_kind(a.kind),
                    size,
                    // Object identity is an intentional bit-pattern
                    // conversion: `u64::MAX` must round-trip exactly, so
                    // it is deliberately NOT range-checked here.
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
        //
        // The order comes from the SHARED sorter (definitionally the
        // preflight order); every ordinal below is bound through a checked
        // cursor whose range the preflight froze — verbatim, never
        // re-derived.
        let app_order = canonical_app_order(app_facts);
        debug_assert_eq!(
            app_order.len(),
            plan.app_len,
            "preflight/write app-fact length diverged"
        );
        let fact_ords = CheckedOrdinals {
            column: "fact_ord",
            len: plan.app_len,
        };
        // For each committed APPLICATION FACT (not per id): input
        // duplicates under one id are distinct facts (conflict
        // preservation — the builder merges commutatively, never by
        // arrival), so each fact persists under its own fact_ord.
        let mut app_stmt = tx.prepare(
            "INSERT INTO app_snapshot_apps
             (run_id, app_id, fact_ord, id_encoding, name, version, publisher,
              install_location, install_date, estimated_size, uninstall_string,
              quiet_uninstall_string, modify_path, install_source, source, kind,
              system_component, bundle_identifier, executable_path, executable_candidate)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                     ?15, ?16, ?17, ?18, ?19, ?20)",
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
        for (fact_pos, &fi) in app_order.iter().enumerate() {
            let fact = &app_facts[fi];
            let r = &fact.record;
            let fact_ord = fact_ords.at(fact_pos);
            // Preflighted verbatim: the estimated size for this canonical
            // position was already checked, so this bind cannot fail —
            // there is no second conversion after mutation begins.
            let estimated_size = plan.estimated_sizes[fact_pos];
            debug_assert!(
                plan.per_app.len() == plan.app_len,
                "preflight per-app plan diverged"
            );
            let per_app = &plan.per_app[fact_pos];
            let prov_ords = CheckedOrdinals {
                column: "prov_ord",
                len: per_app.prov_len,
            };
            let view_ords = CheckedOrdinals {
                column: "view_ord",
                len: per_app.view_len,
            };
            let root_ords = CheckedOrdinals {
                column: "root_ord",
                len: per_app.root_len,
            };
            let evidence_ords = CheckedOrdinals {
                column: "evidence_ord",
                // Every per-artifact group restarts at 0, so the longest
                // group bounds every ordinal this cursor will bind. The
                // group-membership assertion below pins the grouping
                // itself to the preflight.
                len: per_app
                    .evidence_groups
                    .iter()
                    .map(|g| {
                        fact.associations
                            .iter()
                            .filter(|(p, _)| path_bytes(p) == g.as_slice())
                            .count()
                    })
                    .max()
                    .unwrap_or(0)
                    .max(1),
            };
            app_stmt.execute(params![
                run_id.0,
                r.id.0,
                fact_ord,
                // Which identity encoding this id was derived with, so a
                // future migration can tell current ids from legacy ones
                // without guessing (see MIGRATION_V6).
                coresight_apps::ApplicationId::ID_ENCODING_VERSION as i64,
                r.name,
                r.version,
                r.publisher,
                r.install_location
                    .as_ref()
                    .map(|p| crate::path_encoding::encode(p)),
                r.install_date,
                // Preflighted verbatim (see above): no conversion can fail
                // here, after the prior snapshot rows were deleted.
                estimated_size,
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
            debug_assert_eq!(
                prov.len(),
                per_app.prov_len,
                "preflight/write provenance length diverged"
            );
            for (prov_ord, source) in prov.iter().enumerate() {
                let prov_ord = prov_ords.at(prov_ord);
                prov_stmt.execute(params![
                    run_id.0,
                    r.id.0,
                    fact_ord,
                    prov_ord,
                    crate::snapshot_codec::encode_application_source(source.clone()),
                ])?;
            }
            let mut views = r.observed_in_views.clone();
            views.sort();
            views.dedup();
            debug_assert_eq!(
                views.len(),
                per_app.view_len,
                "preflight/write views length diverged"
            );
            for (view_ord, view) in views.iter().enumerate() {
                let view_ord = view_ords.at(view_ord);
                view_stmt.execute(params![run_id.0, r.id.0, fact_ord, view_ord, view,])?;
            }
            let mut roots = fact.install_roots.clone();
            roots.sort_by(|a, b| path_bytes(a).cmp(path_bytes(b)));
            roots.dedup();
            debug_assert_eq!(
                roots.len(),
                per_app.root_len,
                "preflight/write roots length diverged"
            );
            for (root_ord, root) in roots.iter().enumerate() {
                let root_ord = root_ords.at(root_ord);
                root_stmt.execute(params![
                    run_id.0,
                    r.id.0,
                    fact_ord,
                    root_ord,
                    crate::path_encoding::encode(root),
                ])?;
            }
            // Associations: canonical (artifact-path, evidence) order.
            let mut assocs = fact.associations.clone();
            sort_assocs(&mut assocs);
            // The grouping must be the preflight's grouping: same group
            // boundaries in the same order, or the per-group ordinals
            // would not be the preflighted ones.
            {
                let mut groups: Vec<Vec<u8>> = Vec::new();
                for (artifact_path, _) in &assocs {
                    let key = path_bytes(artifact_path).to_vec();
                    if groups.last().is_none_or(|last| *last != key) {
                        groups.push(key);
                    }
                }
                debug_assert_eq!(
                    groups, per_app.evidence_groups,
                    "preflight/write evidence grouping diverged"
                );
            }
            // Group ordinals per artifact path (the PK includes the path).
            let mut per_artifact_ord: std::collections::BTreeMap<Vec<u8>, usize> =
                std::collections::BTreeMap::new();
            for (artifact_path, evidence) in &assocs {
                let key = path_bytes(artifact_path).to_vec();
                let ord = per_artifact_ord.entry(key).or_insert(0);
                let evidence_ord = evidence_ords.at(*ord);
                let (group_tag, group_source) =
                    crate::snapshot_codec::encode_correlation_group(&evidence.correlation_group);
                ev_stmt.execute(params![
                    run_id.0,
                    r.id.0,
                    fact_ord,
                    crate::path_encoding::encode(artifact_path),
                    evidence_ord,
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
            // Footprints for this app. Canonicalized by the SAME
            // normalization + reconciliation the loader applies (see
            // `canonicalize_footprints` and `normalize_candidate_evidence`),
            // so the rows written here are exactly the rows reload
            // reconstructs — no duplicate description is stored and then
            // silently dropped on read.
            let mut footprints = fact.footprints.clone();
            for fp in &mut footprints {
                coresight_apps::footprint::normalize_candidate_evidence(fp);
            }
            canonicalize_footprints(&mut footprints);
            debug_assert_eq!(
                footprints.len(),
                per_app.footprint_evidence_lens.len(),
                "preflight/write footprint length diverged"
            );
            let fp_ords = CheckedOrdinals {
                column: "footprint_ord",
                len: per_app.footprint_evidence_lens.len(),
            };
            for (fp_pos, fp) in footprints.iter().enumerate() {
                let fp_ord = fp_ords.at(fp_pos);
                debug_assert_eq!(
                    fp.evidence.len(),
                    per_app.footprint_evidence_lens[fp_pos],
                    "preflight/write footprint-evidence length diverged"
                );
                let fp_ev_ords = CheckedOrdinals {
                    column: "evidence_ord",
                    len: per_app.footprint_evidence_lens[fp_pos],
                };
                fp_stmt.execute(params![
                    run_id.0,
                    r.id.0,
                    fact_ord,
                    fp_ord,
                    crate::path_encoding::encode(&fp.path),
                    crate::snapshot_codec::encode_footprint_kind(fp.kind),
                    crate::snapshot_codec::encode_app_confidence(fp.confidence),
                ])?;
                // `fp.evidence` was already normalized to the documented
                // SET semantics above, so it is written in canonical order
                // with duplicates removed — exactly what the loader reads
                // back and exactly what the producer published.
                for (ev_pos, e) in fp.evidence.iter().enumerate() {
                    let ev_ord = fp_ev_ords.at(ev_pos);
                    fp_ev_stmt.execute(params![
                        run_id.0,
                        r.id.0,
                        fact_ord,
                        fp_ord,
                        ev_ord,
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
        sort_coverage(&mut coverage);
        debug_assert_eq!(
            coverage.len(),
            plan.coverage_len,
            "preflight/write coverage length diverged"
        );
        let coverage_ords = CheckedOrdinals {
            column: "coverage_ord",
            len: plan.coverage_len,
        };
        {
            let mut stmt = tx.prepare(
                "INSERT INTO app_snapshot_coverage (run_id, coverage_ord, source, status, note)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for (ord, c) in coverage.iter().enumerate() {
                let coverage_ord = coverage_ords.at(ord);
                stmt.execute(params![
                    run_id.0,
                    coverage_ord,
                    c.source,
                    crate::snapshot_codec::encode_source_status(c.status),
                    c.note,
                ])?;
            }
        }

        // ---- Relationships (canonical order → ordinals). ----
        let mut rels = input.relationships.clone();
        sort_relationships(&mut rels);
        debug_assert_eq!(
            rels.len(),
            plan.rel_len,
            "preflight/write relationship length diverged"
        );
        let rel_ords = CheckedOrdinals {
            column: "rel_ord",
            len: plan.rel_len,
        };
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
            for (rel_pos, rel) in rels.iter().enumerate() {
                let rel_ord = rel_ords.at(rel_pos);
                rel_stmt.execute(params![
                    run_id.0,
                    rel_ord,
                    crate::snapshot_codec::encode_relationship_fact_kind(rel.kind),
                    // Bit-pattern conversion (intentional): see artifacts.
                    rel.object.map(|o| o.volume as i64),
                    rel.object.map(|o| o.file_id as i64),
                    rel.object.and_then(|o| o.file_id_hi.map(|hi| hi as i64)),
                    rel.content_sha256,
                ])?;
                let mut members = rel.paths.clone();
                members.sort_by(|a, b| path_bytes(a).cmp(path_bytes(b)));
                debug_assert_eq!(
                    members.len(),
                    plan.rel_member_lens[rel_pos],
                    "preflight/write member length diverged"
                );
                let mem_ords = CheckedOrdinals {
                    column: "member_ord",
                    len: plan.rel_member_lens[rel_pos],
                };
                // Duplicate member paths are kept verbatim under ordinals
                // (the builder dedups canonically on reload — same result).
                for (mem_ord, member) in members.iter().enumerate() {
                    let mem_ord = mem_ords.at(mem_ord);
                    mem_stmt.execute(params![
                        run_id.0,
                        rel_ord,
                        mem_ord,
                        crate::path_encoding::encode(member),
                    ])?;
                }
            }
        }

        // ---- History facts (canonical order → ordinals; conflicting
        // (run, path) rows share keys by design — ordinals keep both). ----
        let mut history = input.history.clone();
        sort_history(&mut history);
        debug_assert_eq!(
            history.len(),
            plan.hist_len,
            "preflight/write history length diverged"
        );
        let hist_ords = CheckedOrdinals {
            column: "hist_ord",
            len: plan.hist_len,
        };
        {
            let mut stmt = tx.prepare(
                "INSERT INTO app_snapshot_history
                 (run_id, hist_ord, hist_run_id, path, device, inode, file_id_hi, category)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            for (ord, h) in history.iter().enumerate() {
                let hist_ord = hist_ords.at(ord);
                stmt.execute(params![
                    run_id.0,
                    hist_ord,
                    h.run_id,
                    crate::path_encoding::encode(&h.path),
                    // Bit-pattern conversion (intentional): see artifacts.
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
    ///
    /// The list itself is capped by `limits` and the caller can detect a
    /// cut short simply: a full `max_results`-length result means there
    /// may be more (the same convention as every other bounded list query
    /// in this store — see `QueryLimits`).
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
        let rows = stmt.query_map(params![probe_limit(limits)?], |r| {
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
        // The probe row (fetched above) proves whether more snapshots
        // exist; it is never published.
        out.truncate(limits.max_results);
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
        // The bound is validated FIRST, before the existence probe, so an
        // unsupported limit is refused even when the run has no snapshot
        // (Phase 6.4.3, Workstream B). Returning `Ok(None)` for an absent
        // run is absence, and must never be how an invalid bound passes.
        limits.to_sql_limit()?;
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
    ///
    /// Returns the records TOGETHER with the sections the caller's bound
    /// cut short, so a partially loaded inventory is visible as such and
    /// can never be read as the complete application set.
    pub fn load_snapshot_applications(
        &self,
        run_id: &RunId,
        limits: &QueryLimits,
    ) -> Result<Option<LoadedApplications>, StoreError> {
        let Some(loaded) = self.load_system_snapshot(run_id, limits)? else {
            return Ok(None);
        };
        Ok(Some(LoadedApplications {
            records: loaded.app_facts.into_iter().map(|f| f.record).collect(),
            load_truncated_sections: loaded.load_truncated_sections,
        }))
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
        let rows = stmt.query_map(params![run_id.0, probe_limit(limits)?], |r| {
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
            "SELECT app_id, fact_ord, id_encoding, name, version, publisher, install_location,
                    install_date, estimated_size, uninstall_string, quiet_uninstall_string,
                    modify_path, install_source, source, kind, system_component,
                    bundle_identifier, executable_path, executable_candidate
             FROM app_snapshot_apps WHERE run_id = ?1
             ORDER BY fact_ord LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![run_id.0, probe_limit(limits)?], |r| {
            Ok(AppRow {
                app_id: r.get(0)?,
                fact_ord: r.get(1)?,
                id_encoding: r.get(2)?,
                name: r.get(3)?,
                version: r.get(4)?,
                publisher: r.get(5)?,
                install_location: r.get(6)?,
                install_date: r.get(7)?,
                estimated_size: r.get(8)?,
                uninstall_string: r.get(9)?,
                quiet_uninstall_string: r.get(10)?,
                modify_path: r.get(11)?,
                install_source: r.get(12)?,
                source: r.get(13)?,
                kind: r.get(14)?,
                system_component: r.get(15)?,
                bundle_identifier: r.get(16)?,
                executable_path: r.get(17)?,
                executable_candidate: r.get(18)?,
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
            let ctx = ReadCtx {
                run_id,
                limits,
                run_tag,
            };
            // Provenance: strictly decoded enums, canonically ordered and
            // deduplicated (the same union rule the builder applies). A
            // value this build cannot decode is corruption, never a
            // silently dropped source.
            let mut provenance: Vec<ApplicationSource> = Vec::new();
            for raw in self.load_child_strings(
                ChildStrings {
                    section: "provenance",
                    table: "app_snapshot_provenance",
                    column: "source",
                    ord_column: "prov_ord",
                },
                ctx,
                &app_id,
                fact_ord,
                caps,
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
                    section: "views",
                    table: "app_snapshot_views",
                    column: "view",
                    ord_column: "view_ord",
                },
                ctx,
                &app_id,
                fact_ord,
                caps,
            )?;
            observed_in_views.sort();
            observed_in_views.dedup();
            record.observed_in_views = observed_in_views;
            let mut install_roots = Vec::new();
            for raw in self.load_child_strings(
                ChildStrings {
                    section: "install_roots",
                    table: "app_snapshot_roots",
                    column: "path",
                    ord_column: "root_ord",
                },
                ctx,
                &app_id,
                fact_ord,
                caps,
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
    ///
    /// A cap hit is enforced AND reported like every other section: these
    /// are model-affecting inputs (install roots drive containment, and
    /// provenance is part of the application fact), so a silent prefix
    /// here would let a partial load become a model.
    fn load_child_strings(
        &self,
        src: ChildStrings,
        ctx: ReadCtx<'_>,
        app_id: &str,
        fact_ord: i64,
        caps: &mut LoadCaps,
    ) -> Result<Vec<String>, StoreError> {
        let ChildStrings {
            section,
            table,
            column,
            ord_column,
        } = src;
        let ReadCtx {
            run_id,
            limits,
            run_tag,
        } = ctx;
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
                params![run_id.0, app_id, fact_ord, probe_limit(limits)?],
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
        Ok(enforce_cap(section, limits.max_results, out, caps))
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
            params![run_id.0, app_id, fact_ord, probe_limit(limits)?],
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
            params![run_id.0, app_id, fact_ord, probe_limit(limits)?],
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
                params![run_id.0, app_id, fact_ord, fp_ord, probe_limit(limits)?],
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
            // The stored cap is applied BEFORE normalization: if the
            // section exceeded the query bound, the truncation signal must
            // be preserved (a rebuild refuses incomplete facts), and
            // normalizing first could hide duplicate rows inside the bound
            // and mask a real overflow.
            let evidence = enforce_cap("footprint_evidence", limits.max_results, evidence, caps);
            // Then the capped evidence is normalized to the documented SET
            // contract — the SAME `normalize_candidate_evidence` the
            // producer applies at admission and the commit applies on
            // write. A tampered store that inserted duplicate evidence rows
            // therefore reloads as the canonical set it should have stored,
            // rather than as a list that violates the contract the rest of
            // the code promises.
            let evidence = normalize_evidence_set(evidence);
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
        let rows = stmt.query_map(params![run_id.0, probe_limit(limits)?], |r| {
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
        let rows = stmt.query_map(params![run_id.0, probe_limit(limits)?], |r| {
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
                .query_map(params![run_id.0, rel_ord, probe_limit(limits)?], |r| {
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
        let rows = stmt.query_map(params![run_id.0, probe_limit(limits)?], |r| {
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
    /// Which identity encoding this row's `app_id` was derived with
    /// (`ApplicationId::ID_ENCODING_VERSION`). A row claiming the legacy
    /// encoding in a v6 store means the migration did not run (or was
    /// bypassed); it is refused rather than silently reinterpreted.
    id_encoding: i64,
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
    // The stored id must be the CURRENT encoding's id for this row's own
    // (name, publisher) — `verify_application_id` already proves the
    // normalized pair matches, so this check proves the encoding. A row
    // claiming another encoding in a v6 store was written by a build this
    // one cannot interpret, so it is refused rather than trusted.
    if row.id_encoding != coresight_apps::ApplicationId::ID_ENCODING_VERSION as i64 {
        return Err(corrupt(
            "app_snapshot_apps",
            "id_encoding",
            run_tag,
            format!(
                "application id was derived with encoding version {}, but this build \
                 derives version {}; re-keying requires the forward migration",
                row.id_encoding,
                coresight_apps::ApplicationId::ID_ENCODING_VERSION
            ),
        ));
    }
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

/// Convert a domain `u64` to the store's signed `INTEGER` domain,
/// rejecting anything above `i64::MAX` (Workstream C). Deliberately NOT
/// used for object identity: those are intentional bit-pattern
/// conversions (see [`snap_identity`] and the identity write path).
///
/// ## Caller input, not persisted corruption (Phase 6.4.6)
///
/// Every caller of these helpers passes a value the CALLER handed the
/// commit (`Inventory` / `FootprintReport` counters, artifact and
/// estimated sizes), so an unrepresentable value is reported as
/// [`StoreError::InvalidSnapshotInput`] — the caller-input half of the
/// error contract — never as persisted corruption. A value the caller
/// handed us is never evidence of database damage.
pub(crate) fn checked_u64_i64(
    value: Option<u64>,
    table: &'static str,
    column: &'static str,
    run_id: &Option<String>,
) -> Result<Option<i64>, StoreError> {
    match value {
        None => Ok(None),
        Some(v) => {
            let converted = i64::try_from(v).map_err(|_| StoreError::InvalidSnapshotInput {
                table,
                column,
                run_id: run_id.clone(),
                value: v as u128,
            })?;
            Ok(Some(converted))
        }
    }
}

/// Convert a domain `u64` counter to the store's signed `INTEGER` domain,
/// rejecting values above `i64::MAX`.
///
/// The counters are caller-supplied (`Inventory` / `FootprintReport`),
/// so an unrepresentable value is caller input
/// ([`StoreError::InvalidSnapshotInput`]), not persisted corruption.
pub(crate) fn checked_counter_i64(
    value: u64,
    table: &'static str,
    column: &'static str,
    run_id: &Option<String>,
) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::InvalidSnapshotInput {
        table,
        column,
        run_id: run_id.clone(),
        value: value as u128,
    })
}

/// Convert an ordinal (an index into a canonically ordered collection)
/// to the store's signed `INTEGER` domain, rejecting values above
/// `i64::MAX`. No giant allocation is needed to hit the bound: the
/// caller's collection is already in memory, so this is a cheap check.
///
/// An ordinal is derived from the caller's collection shape during a
/// commit, so an unrepresentable one is caller input
/// ([`StoreError::InvalidSnapshotInput`] — the value is recorded in the
/// `u128` domain so no `usize` width is ever narrowed), not persisted
/// corruption.
pub(crate) fn checked_ord_i64(
    ordinal: usize,
    table: &'static str,
    column: &'static str,
    run_id: &Option<String>,
) -> Result<i64, StoreError> {
    i64::try_from(ordinal).map_err(|_| StoreError::InvalidSnapshotInput {
        table,
        column,
        run_id: run_id.clone(),
        value: ordinal as u128,
    })
}

/// The two application vectors the commit accepts disagreed. Reported as
/// corruption because the caller would otherwise get a snapshot that does
/// not describe the facts the model was built from.
fn parallel_mismatch(run_id: &RunId, field: &str) -> StoreError {
    StoreError::Corrupt {
        table: "app_snapshot_apps",
        column: "app_id",
        run_id: Some(run_id.0.clone()),
        detail: format!(
            "application facts are not parallel to the committed model input ({field} differs)"
        ),
    }
}

/// Canonicalize a footprint-candidate collection: deterministic order,
/// and a COMMUTATIVE reconciliation of same-key duplicates — never an
/// arrival-order winner.
///
/// ## Set semantics (explicit)
///
/// The admission key is `(path bytes, application, kind)`: that is what
/// makes one *scope* for an application, so it is the unit a caller
/// reports once. Everything else — confidence and the evidence list —
/// describes how well that scope is known, so two candidates sharing a
/// key are two DESCRIPTIONS of one scope, not two scopes.
///
/// ## Reconciliation rule (commutative, deterministic, producer-consistent)
///
/// Same-key candidates are reconciled by the SAME precedence the
/// producer (`coresight_apps::footprint::admit_candidate` /
/// `candidate_rank`) applies: stronger `Confidence` wins, then the
/// canonically larger evidence list. Because the comparison is a total
/// order over the candidates' own content, `choose(a, b) == choose(b, a)`
/// — the surviving candidate is a pure function of the duplicate SET, so
/// no arrival order can decide it and no meaningful evidence is silently
/// dropped (the winner is simply the strictly better description).
///
/// Candidates that differ ONLY in confidence/evidence are therefore one
/// fact; candidates that differ in path, application, or kind are
/// distinct facts and are all preserved.
fn canonicalize_footprints(candidates: &mut Vec<FootprintCandidate>) {
    // Total order over the full content: sort by the admission key, then
    // by the reconciliation rank in DESCENDING order, so within each
    // same-key run the FIRST element is the best description.
    candidates.sort_by(|a, b| {
        path_bytes(&a.path)
            .cmp(path_bytes(&b.path))
            .then(a.app.0.cmp(&b.app.0))
            .then(a.kind.cmp(&b.kind))
            .then(evidence_rank(b).cmp(&evidence_rank(a)))
    });
    // `dedup_by` keeps the FIRST element of each adjacent same-key run,
    // which the sort above made the best description. Equivalent to a max
    // over the group, so the survivor cannot depend on the pre-sort order.
    candidates.dedup_by(|a, b| a.path == b.path && a.app == b.app && a.kind == b.kind);
}

/// The reconciliation rank of one candidate — delegated to THE one
/// policy in `coresight_apps::footprint::candidate_rank`, so the producer
/// and persistence can never disagree about which description of a scope
/// wins.
///
/// The policy is: stronger confidence, then the larger count of DISTINCT
/// evidence items, then the canonically-ordered evidence content as a
/// deterministic tie-breaker. `FootprintEvidence` is a SET (`Ord`,
/// dedup-after-sort), which is stated in [`normalize_candidate_evidence`].
fn evidence_rank(c: &FootprintCandidate) -> (u8, usize, Vec<coresight_apps::FootprintEvidence>) {
    coresight_apps::footprint::candidate_rank(c)
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

/// Which normalized child table to read: the section name reported when
/// the caller's cap cuts this read short, the table name, its value
/// column, and its ordinal column. Grouping the names keeps the loader
/// signatures small and makes the table/column pair that owns any
/// corruption impossible to mix up.
#[derive(Debug, Clone, Copy)]
struct ChildStrings {
    section: &'static str,
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

/// Normalize a decoded footprint-evidence list to the documented SET
/// contract: canonical ordering, duplicates removed (Phase 6.4.3,
/// Workstream C).
///
/// This is the READ-side counterpart of the write-side normalization, and
/// it goes through the one shared normalizer
/// (`coresight_apps::footprint::normalize_candidate_evidence`) so producer,
/// commit, and reload cannot disagree about what an evidence set is.
///
/// The chosen tampered-store policy is NORMALIZE, not reject: the write
/// path already guarantees a legitimate database never contains duplicate
/// evidence rows (they are deduplicated on write), so duplicates can only
/// arrive from tampering or corruption. Normalizing them away preserves
/// the set contract the callers rely on, keeps cap detection intact (the
/// cap is applied first, so a genuinely oversized section still reports
/// truncation), and avoids turning a duplicated row into a hard read
/// failure for historical data that would otherwise be readable.
fn normalize_evidence_set(
    evidence: Vec<coresight_apps::FootprintEvidence>,
) -> Vec<coresight_apps::FootprintEvidence> {
    let mut scratch = FootprintCandidate {
        path: PathBuf::new(),
        app: ApplicationId(String::new()),
        kind: coresight_apps::FootprintKind::InstallationDirectory,
        confidence: coresight_apps::Confidence::Confirmed,
        evidence,
    };
    coresight_apps::footprint::normalize_candidate_evidence(&mut scratch);
    scratch.evidence
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

/// The per-read context threaded through a section loader: which run,
/// under which bound, and the diagnostic tag for corruption messages.
/// Bundled (and `Copy`) so the loader signatures stay small and no call
/// site can pass a mismatched run/limit pair. Cap bookkeeping travels
/// separately as `&mut LoadCaps`.
#[derive(Clone, Copy)]
struct ReadCtx<'a> {
    run_id: &'a RunId,
    limits: &'a QueryLimits,
    run_tag: &'a Option<String>,
}

/// The shared probe limit: one row beyond the caller's bound, so a cap hit
/// is provable. Delegates to the ONE conversion policy
/// ([`QueryLimits::to_probe_sql_limit`]) so this layer and the older
/// history queries can never disagree about what a limit means — and so a
/// bound too large to represent fails loudly instead of silently becoming
/// an unbounded query.
fn probe_limit(limits: &QueryLimits) -> Result<i64, StoreError> {
    limits.to_probe_sql_limit()
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

/// One frozen numeric value per persisted INTEGER that the write path
/// cannot recompute fallibly: artifact sizes are checked `u64` domains,
/// ordinals are checked `usize` collection positions. The write path
/// binds these verbatim and asserts the binding matches the plan, so no
/// second conversion can fail after mutation begins.
///
/// (Phase 6.4.6, Workstream B: the validated persistence representation.
/// Estimated sizes ride the same helper through the same preflight; they
/// are read off `app_facts` at bind time but were already checked here,
/// so their bind is verbatim too — see `preflight_snapshot_plan`.)
struct SnapshotNumericPlan {
    /// Checked sizes for the canonically-sorted artifacts, in write order.
    artifact_sizes: Vec<Option<i64>>,
    /// Checked estimated sizes, in canonical FACT order (position `i`
    /// belongs to the fact the write loop visits at position `i`).
    estimated_sizes: Vec<Option<i64>>,
    /// Ordinals survive as positions: every ordinal is simply
    /// `0..len` of its canonical collection, and the lengths are frozen
    /// here. The write loop binds the checked value for the CURRENT
    /// position (see `CheckedOrdinals::at`); the lengths prove every
    /// ordinal the write path will ever ask for was checked in preflight.
    artifact_len: usize,
    app_len: usize,
    coverage_len: usize,
    rel_len: usize,
    rel_member_lens: Vec<usize>,
    hist_len: usize,
    per_app: Vec<PerAppPlan>,
}

/// The numeric shape of one application fact's nested collections: frozen
/// lengths the write path iterates, so every nested ordinal
/// (provenance, view, root, per-artifact evidence, footprint,
/// footprint-evidence) was checked before any mutation.
struct PerAppPlan {
    prov_len: usize,
    view_len: usize,
    root_len: usize,
    /// For `app_snapshot_evidence`: the association list is grouped per
    /// artifact path, and each group's evidence ordinals run
    /// `0..group_len`. The group boundaries are path-dependent, so the
    /// plan stores the sorted+grouped artifact paths (byte keys) in full:
    /// the write path asserts it regroups to the identical groups, then
    /// binds the checked ordinal for the current position.
    evidence_groups: Vec<Vec<u8>>,
    /// For `app_snapshot_footprints`: per-candidate normalized evidence
    /// lengths, in canonical footprint order (footprint ordinals are
    /// `0..footprints.len()`).
    footprint_evidence_lens: Vec<usize>,
}

/// A cursor over one PRE-CHECKED ordinal domain.
///
/// `at(i)` returns the checked `i64` for position `i`. Because the
/// preflight already proved every ordinal in `0..len` is representable,
/// `at` does NOT re-run the conversion: it returns the position's value
/// from a range the plan has already frozen, and refuses a position
/// outside that range (fail-closed rather than silently clamping or
/// wrapping). Re-converting inside `at` would have been a second
/// fallible conversion after mutation begins, which this phase forbids.
struct CheckedOrdinals {
    /// Diagnostic-only label for the ordinal domain this cursor serves
    /// (`artifact_ord`, `prov_ord`, …). The domain's TABLE and COLUMN are
    /// reported by the PREFLIGHT, which is where a rejected value is
    /// actually diagnosed — `at()` cannot fail, so it never constructs an
    /// error and never needs to name them.
    column: &'static str,
    len: usize,
}

impl CheckedOrdinals {
    /// The ordinal `pos`, already proven representable by the preflight.
    ///
    /// `debug_assert!` guards the frozen range in test/debug builds; in
    /// release the value is returned as-is because the preflight's bound
    /// check already established representability for the whole `0..len`
    /// domain, and `pos` is by construction a position inside it.
    fn at(&self, pos: usize) -> i64 {
        debug_assert!(
            pos < self.len,
            "write path asked for ordinal {pos} outside the preflighted {} range (len {})",
            self.column,
            self.len
        );
        pos as i64
    }
}

/// Freeze every numeric conversion `commit_system_snapshot` will need,
/// from the SAME canonically-sorted, normalized collections the write
/// path persists — before any transaction opens or any row is deleted.
///
/// What is checked here (and therefore CANNOT fail later):
/// - every artifact `size` and every application `estimated_size_bytes`
///   (checked `u64` domains; `None` passes through as absent);
/// - every ordinal the write loops will bind: artifact, fact,
///   provenance, view, root, per-artifact evidence, footprint,
///   footprint-evidence, coverage, relationship, member, history.
///   An ordinal is `0..len` of a caller-sized collection, so checking
///   the BOUND (`len - 1` fits) checks every ordinal the loop can bind.
///   A collection no caller can make large enough to overflow
///   (`usize::MAX`-ish) is still rejected here rather than trusted —
///   the check is O(1) per collection.
///
/// What is deliberately NOT converted: object-identity device / inode /
/// high-bit columns (intentional full-width bit-pattern storage), the
/// `id_encoding` version probe (a small constant), and the
/// `system_component` flag (a 0/1 literal). Strict READ-side decoding
/// (`u64_nonneg`, `snap_identity`, unknown-enum rejection) is untouched:
/// persisted-malformed data stays [`StoreError::Corrupt`].
fn preflight_snapshot_plan(
    input: &SystemModelInput,
    app_facts: &[AppSnapshotFact],
    run_tag: &Option<String>,
) -> Result<SnapshotNumericPlan, StoreError> {
    // Artifacts, in the write path's canonical order.
    let mut artifacts = input.artifacts.clone();
    sort_artifacts(&mut artifacts);
    let mut artifact_sizes = Vec::with_capacity(artifacts.len());
    for a in &artifacts {
        artifact_sizes.push(checked_u64_i64(
            a.size,
            "app_snapshot_artifacts",
            "size",
            run_tag,
        )?);
    }
    // Ordinal bounds: checking `len - 1` fits checks every `0..len` the
    // write loop can bind. `checked_ord_i64(0, …)` also pins the empty
    // case (len 0) through the same helper, so the empty collection is
    // preflighted, not assumed.
    let artifact_len = artifacts.len();
    check_ord_bound(
        artifact_len,
        "app_snapshot_artifacts",
        "artifact_ord",
        run_tag,
    )?;

    // Applications, in the write path's canonical fact order, with every
    // nested collection normalized EXACTLY as the write path normalizes
    // it (sort/dedup/normalize/reconcile), so the frozen lengths are the
    // lengths the write loop will iterate.
    let app_order = canonical_app_order(app_facts);
    let app_len = app_facts.len();
    check_ord_bound(app_len, "app_snapshot_apps", "fact_ord", run_tag)?;
    let mut per_app = Vec::with_capacity(app_len);
    let mut estimated_sizes = Vec::with_capacity(app_len);
    for &fi in &app_order {
        let fact = &app_facts[fi];
        // Estimated size: checked now, bound verbatim later.
        estimated_sizes.push(checked_u64_i64(
            fact.record.estimated_size_bytes,
            "app_snapshot_apps",
            "estimated_size",
            run_tag,
        )?);
        let mut prov = fact.record.provenance.clone();
        prov.sort();
        prov.dedup();
        check_ord_bound(prov.len(), "app_snapshot_provenance", "prov_ord", run_tag)?;
        let mut views = fact.record.observed_in_views.clone();
        views.sort();
        views.dedup();
        check_ord_bound(views.len(), "app_snapshot_views", "view_ord", run_tag)?;
        let mut roots = fact.install_roots.clone();
        roots.sort_by(|a, b| path_bytes(a).cmp(path_bytes(b)));
        roots.dedup();
        check_ord_bound(roots.len(), "app_snapshot_roots", "root_ord", run_tag)?;
        // Associations grouped per artifact path, exactly as the write
        // path groups them (canonical artifact-path, evidence order;
        // per-path ordinals restart at 0).
        let mut assocs = fact.associations.clone();
        sort_assocs(&mut assocs);
        let mut evidence_groups: Vec<Vec<u8>> = Vec::new();
        for (artifact_path, _) in &assocs {
            let key = path_bytes(artifact_path).to_vec();
            if evidence_groups.last().is_none_or(|last| *last != key) {
                evidence_groups.push(key);
            }
        }
        // Every group's ordinals run 0..(group length); the longest
        // group bounds them all, and each group length is also checked
        // so a single group's overflow cannot hide behind a short max.
        let mut group_len = 0usize;
        for group in &evidence_groups {
            let len = assocs
                .iter()
                .filter(|(p, _)| path_bytes(p) == group.as_slice())
                .count();
            check_ord_bound(len, "app_snapshot_evidence", "evidence_ord", run_tag)?;
            group_len = group_len.max(len);
        }
        let _ = group_len;
        // Footprints normalized + reconciled exactly as the write path
        // does (shared normalizer + canonicalizer).
        let mut footprints = fact.footprints.clone();
        for fp in &mut footprints {
            coresight_apps::footprint::normalize_candidate_evidence(fp);
        }
        canonicalize_footprints(&mut footprints);
        check_ord_bound(
            footprints.len(),
            "app_snapshot_footprints",
            "footprint_ord",
            run_tag,
        )?;
        let mut footprint_evidence_lens = Vec::with_capacity(footprints.len());
        for fp in &footprints {
            check_ord_bound(
                fp.evidence.len(),
                "app_snapshot_footprint_evidence",
                "evidence_ord",
                run_tag,
            )?;
            footprint_evidence_lens.push(fp.evidence.len());
        }
        per_app.push(PerAppPlan {
            prov_len: prov.len(),
            view_len: views.len(),
            root_len: roots.len(),
            evidence_groups,
            footprint_evidence_lens,
        });
    }

    // Coverage, relationships (+ member lengths), history — same pattern:
    // canonical sort, frozen length, bound check.
    let mut coverage = input.source_coverage.clone();
    sort_coverage(&mut coverage);
    let coverage_len = coverage.len();
    check_ord_bound(
        coverage_len,
        "app_snapshot_coverage",
        "coverage_ord",
        run_tag,
    )?;
    let mut rels = input.relationships.clone();
    sort_relationships(&mut rels);
    let rel_len = rels.len();
    check_ord_bound(rel_len, "app_snapshot_relationships", "rel_ord", run_tag)?;
    let mut rel_member_lens = Vec::with_capacity(rel_len);
    for rel in &rels {
        let mut members = rel.paths.clone();
        members.sort_by(|a, b| path_bytes(a).cmp(path_bytes(b)));
        check_ord_bound(
            members.len(),
            "app_snapshot_rel_members",
            "member_ord",
            run_tag,
        )?;
        rel_member_lens.push(members.len());
    }
    let mut history = input.history.clone();
    sort_history(&mut history);
    let hist_len = history.len();
    check_ord_bound(hist_len, "app_snapshot_history", "hist_ord", run_tag)?;

    Ok(SnapshotNumericPlan {
        artifact_sizes,
        estimated_sizes,
        artifact_len,
        app_len,
        coverage_len,
        rel_len,
        rel_member_lens,
        hist_len,
        per_app,
    })
}

/// Check that every ordinal `0..len` of one write collection fits the
/// signed `INTEGER` domain, by checking the largest (`len - 1`).
/// Length 0 binds no ordinal; it is still pinned through the helper so
/// the empty case is preflighted, not assumed.
fn check_ord_bound(
    len: usize,
    table: &'static str,
    column: &'static str,
    run_tag: &Option<String>,
) -> Result<(), StoreError> {
    if len == 0 {
        checked_ord_i64(0, table, column, run_tag)?;
    } else {
        checked_ord_i64(len - 1, table, column, run_tag)?;
    }
    Ok(())
}

/// Canonical artifact order shared by the preflight and the write path:
/// extracted so the two cannot diverge.
fn sort_artifacts(artifacts: &mut [ArtifactFact]) {
    artifacts.sort_by(|a, b| {
        path_bytes(&a.path)
            .cmp(path_bytes(&b.path))
            .then(a.kind.cmp(&b.kind))
            .then(a.size.cmp(&b.size))
            .then(identity_key(&a.identity).cmp(&identity_key(&b.identity)))
    });
}

/// Canonical app-fact order shared by the preflight and the write path.
fn canonical_app_order(app_facts: &[AppSnapshotFact]) -> Vec<usize> {
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
    app_order
}

/// Canonical association order shared by the preflight and the write path.
fn sort_assocs(assocs: &mut [(PathBuf, OwnershipEvidence)]) {
    assocs.sort_by(|a, b| path_bytes(&a.0).cmp(path_bytes(&b.0)).then(a.1.cmp(&b.1)));
}

/// Canonical coverage order shared by the preflight and the write path.
fn sort_coverage(coverage: &mut [SourceCoverage]) {
    coverage.sort_by(|a, b| {
        a.source
            .cmp(&b.source)
            .then(a.status.cmp(&b.status))
            .then(a.note.cmp(&b.note))
    });
}

/// Canonical relationship order shared by the preflight and the write path.
fn sort_relationships(rels: &mut [RelationshipFact]) {
    rels.sort_by(|a, b| {
        rel_kind_key(a.kind)
            .cmp(&rel_kind_key(b.kind))
            .then(identity_key(&a.object).cmp(&identity_key(&b.object)))
            .then(a.content_sha256.cmp(&b.content_sha256))
            .then(sorted_path_keys(&a.paths).cmp(&sorted_path_keys(&b.paths)))
    });
}

/// Canonical history order shared by the preflight and the write path.
fn sort_history(history: &mut [HistoryFact]) {
    history.sort_by(|a, b| {
        a.run_id
            .cmp(&b.run_id)
            .then(path_bytes(&a.path).cmp(path_bytes(&b.path)))
            .then(identity_key(&a.identity).cmp(&identity_key(&b.identity)))
            .then(a.category.cmp(&b.category))
    });
}

fn flatten_footprints(app_facts: &[AppSnapshotFact]) -> Vec<FootprintCandidate> {
    let mut out: Vec<FootprintCandidate> = app_facts
        .iter()
        .flat_map(|f| f.footprints.iter().cloned())
        .collect();
    canonicalize_footprints(&mut out);
    out
}

#[cfg(test)]
mod preflight_tests {
    use super::*;

    const RUN: Option<String> = None;

    fn run_tag() -> Option<String> {
        Some("preflight-test-run".to_string())
    }

    // ---- ordinal conversion boundaries -----------------------------------
    //
    // Ordinals are `0..len` of a caller-sized collection, so no realistic
    // allocation can overflow `i64`. The conversion is therefore tested
    // DIRECTLY at its boundary rather than through an impractical
    // collection: `i64::MAX` is the last representable ordinal and
    // `i64::MAX + 1` must be rejected as invalid input.

    #[test]
    fn an_ordinal_at_the_signed_boundary_is_accepted() {
        assert_eq!(
            checked_ord_i64(
                i64::MAX as usize,
                "app_snapshot_artifacts",
                "artifact_ord",
                &run_tag()
            )
            .unwrap(),
            i64::MAX,
        );
        assert_eq!(
            checked_ord_i64(0, "app_snapshot_apps", "fact_ord", &run_tag()).unwrap(),
            0
        );
    }

    #[test]
    fn an_ordinal_past_the_signed_boundary_is_invalid_input_not_corruption() {
        let err = checked_ord_i64(
            (i64::MAX as u128 + 1) as usize,
            "app_snapshot_apps",
            "fact_ord",
            &run_tag(),
        )
        .expect_err("an ordinal above i64::MAX is unrepresentable");
        match err {
            StoreError::InvalidSnapshotInput {
                table,
                column,
                run_id,
                value,
            } => {
                assert_eq!(table, "app_snapshot_apps");
                assert_eq!(column, "fact_ord");
                assert_eq!(run_id, run_tag());
                assert_eq!(value, i64::MAX as u128 + 1);
            }
            other => panic!("an oversized ordinal must be invalid input, got {other:?}"),
        }
    }

    #[test]
    fn a_whole_domain_ordinal_reports_the_true_value_without_narrowing() {
        // A `u64` at its own maximum must be reported exactly, not
        // truncated to fit the error's display domain.
        let err = checked_u64_i64(Some(u64::MAX), "app_snapshot_artifacts", "size", &run_tag())
            .expect_err("u64::MAX bytes is unrepresentable");
        match err {
            StoreError::InvalidSnapshotInput { value, .. } => {
                assert_eq!(
                    value,
                    u64::MAX as u128,
                    "the rejected value must survive exactly"
                );
            }
            other => panic!("an oversized size must be invalid input, got {other:?}"),
        }
    }

    #[test]
    fn the_preflight_bound_check_rejects_an_unrepresentable_collection() {
        // The preflight checks the collection's BOUND, so a caller-sized
        // collection that could never be allocated is still refused here
        // rather than trusted.
        let impossible = (i64::MAX as u128 + 2) as usize;
        let err = check_ord_bound(
            impossible,
            "app_snapshot_evidence",
            "evidence_ord",
            &run_tag(),
        )
        .expect_err("a collection past the signed ordinal domain must be refused");
        assert!(
            matches!(err, StoreError::InvalidSnapshotInput { .. }),
            "expected invalid input, got {err:?}"
        );
    }

    #[test]
    fn the_preflight_bound_check_accepts_the_empty_and_normal_collections() {
        // The empty collection pins ordinal 0 through the same helper, so
        // it is preflighted rather than assumed safe.
        assert!(check_ord_bound(0, "app_snapshot_coverage", "coverage_ord", &run_tag()).is_ok());
        assert!(check_ord_bound(1, "app_snapshot_coverage", "coverage_ord", &run_tag()).is_ok());
        // i64::MAX elements means the last ordinal is i64::MAX - 1: fine.
        assert!(check_ord_bound(
            i64::MAX as usize,
            "app_snapshot_history",
            "hist_ord",
            &run_tag()
        )
        .is_ok());
    }

    #[test]
    fn counters_report_caller_values_as_invalid_input() {
        let err = checked_counter_i64(
            u64::MAX,
            "app_snapshot_meta",
            "records_truncated",
            &run_tag(),
        )
        .expect_err("an oversized counter is unrepresentable");
        match err {
            StoreError::InvalidSnapshotInput {
                table,
                column,
                run_id,
                value,
            } => {
                assert_eq!(table, "app_snapshot_meta");
                assert_eq!(column, "records_truncated");
                assert_eq!(run_id, run_tag());
                assert_eq!(value, u64::MAX as u128);
            }
            other => panic!("an oversized counter must be invalid input, got {other:?}"),
        }
    }

    // ---- the preflight runs BEFORE any mutation --------------------------

    #[test]
    fn the_preflight_is_reached_before_the_write_transaction_opens() {
        // Ordering proof: with NO valid run in the store, a commit that
        // fails preflight must report the INVALID INPUT, never
        // `UnknownRun`. `UnknownRun` can only be produced by the existence
        // check that runs INSIDE the transaction, so observing the input
        // error instead proves the preflight rejected the commit before
        // the transaction opened and before any statement ran.
        let dir = tempfile::tempdir().unwrap();
        let mut store = HistoryStore::open(&dir.path().join("ordering.db")).unwrap();
        let input = SystemModelInput {
            artifacts: vec![ArtifactFact {
                path: std::path::PathBuf::from("/huge"),
                kind: coresight_apps::ProbedKind::File,
                identity: None,
                content_sha256: None,
                size: Some(u64::MAX),
                access: coresight_capabilities::access::AccessState::ReadSucceeded,
                classification: None,
            }],
            applications: Vec::new(),
            relationships: Vec::new(),
            history: Vec::new(),
            source_coverage: Vec::new(),
        };
        let run_id = crate::RunId("no-such-run".to_string());
        let err = store
            .commit_system_snapshot(
                &run_id,
                &input,
                &[],
                &coresight_apps::Inventory::default(),
                &coresight_apps::FootprintReport::default(),
            )
            .expect_err("the oversize must be rejected");
        match err {
            StoreError::InvalidSnapshotInput { .. } => {}
            other => panic!(
                "the preflight must reject before the transaction's run check; got {other:?}"
            ),
        }
    }

    #[test]
    fn the_preflight_plan_freezes_every_ordinal_domain_before_write() {
        // The plan must be derivable (and total) for a realistic input,
        // and every frozen length must be a bound the write loop honours.
        let dir = tempfile::tempdir().unwrap();
        let mut store = HistoryStore::open(&dir.path().join("plan.db")).unwrap();
        store
            .begin_run(&crate::model::RunRecord {
                run_id: crate::RunId("plan-run".to_string()),
                started_at: std::time::UNIX_EPOCH + std::time::Duration::from_secs(10),
                completed_at: Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(20)),
                roots: vec![std::path::PathBuf::from("/scope")],
                platform: "test".to_string(),
                config: crate::model::ConfigFingerprint::current(),
                status: crate::model::RunStatus::Completed,
                counts: crate::model::RunCounts::default(),
            })
            .unwrap();
        let input = SystemModelInput {
            artifacts: vec![ArtifactFact {
                path: std::path::PathBuf::from("/a"),
                kind: coresight_apps::ProbedKind::File,
                identity: None,
                content_sha256: None,
                size: Some(4096),
                access: coresight_capabilities::access::AccessState::ReadSucceeded,
                classification: None,
            }],
            applications: Vec::new(),
            relationships: Vec::new(),
            history: Vec::new(),
            source_coverage: vec![],
        };
        let plan = preflight_snapshot_plan(&input, &[], &run_tag()).unwrap();
        assert_eq!(plan.artifact_len, 1);
        assert_eq!(plan.artifact_sizes, vec![Some(4096)]);
        assert!(plan.per_app.is_empty());
        assert_eq!(plan.coverage_len, 0);
        assert_eq!(plan.rel_len, 0);
        assert_eq!(plan.hist_len, 0);
        // The cursor binds a pre-checked ordinal without converting again.
        let ords = CheckedOrdinals {
            column: "artifact_ord",
            len: plan.artifact_len,
        };
        assert_eq!(ords.at(0), 0);
    }

    #[test]
    fn an_absent_size_stays_absent_through_the_preflight() {
        let input = SystemModelInput {
            artifacts: vec![ArtifactFact {
                path: std::path::PathBuf::from("/a"),
                kind: coresight_apps::ProbedKind::File,
                identity: None,
                content_sha256: None,
                size: None,
                access: coresight_capabilities::access::AccessState::ReadSucceeded,
                classification: None,
            }],
            applications: Vec::new(),
            relationships: Vec::new(),
            history: Vec::new(),
            source_coverage: Vec::new(),
        };
        let plan = preflight_snapshot_plan(&input, &[], &run_tag()).unwrap();
        assert_eq!(
            plan.artifact_sizes,
            vec![None],
            "an absent size must stay absent, never become zero"
        );
        let _ = RUN;
    }
}
