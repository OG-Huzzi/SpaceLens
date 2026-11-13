//! The platform-neutral dispatcher: `Command` in, `Envelope<Response>` out.
//!
//! Every command is served from registered session facts or refused with a
//! typed error. There is no sample data, no placeholder response, and no
//! silent zero:
//!
//! * `startScan` / `cancelScan` need a [`ScanBackend`](crate::ScanBackend).
//!   None is attached in this build, so both return `SCAN_UNAVAILABLE`.
//! * `getHistory` needs a history store. None is attached, so it returns
//!   `HISTORY_UNAVAILABLE`.
//! * `confirmPlan` runs the *real* `coresight_cleanup` validator under
//!   `ExecutionPolicy::CURRENT_BUILD` (read-only only). Every cleanup plan is
//!   reversible, so it is **correctly** rejected with
//!   `PLAN_REJECTED_UNSAFE` and the validator's reasons. `executePlan`
//!   refuses with the same typed rejection. This is the safety contract
//!   working, not a bug.
//!
//! Category and opportunity order is deterministic: `Category::ALL` order
//! for categories, the planner's order for opportunities, request order for
//! preview items. Repeated commands against an unchanged session serialize
//! byte-identically.

use std::collections::BTreeSet;

use coresight_classifier::Category;
use coresight_cleanup::{
    resolve_plan, validate_plan, CleanupItem, CleanupPlan, ResolveError, ResolveOptions,
    SafetyTier, TargetKind,
};
use coresight_history::encode_path;

use crate::command::{
    CancelScanRequest, Command, ConfirmPlanRequest, ExecutePlanRequest, GetCategoriesRequest,
    GetDriveSummaryRequest, GetHistoryRequest, GetOpportunitiesRequest, PreviewPlanRequest,
    StartScanRequest,
};
use crate::error::{codes, detail_token, ApiError, Envelope};
use crate::response::{
    Category as CategoryResponse, ConfirmPlanResponse, DriveSummary, Opportunity,
    OpportunityPreview, OpportunityTier, PlanPreview, PlanPreviewItem, PreviewTargetKind, Response,
    ScanStatus, ScanSummaryRef, ScanTotals,
};
use crate::session::{opportunity_id_for, ScanState, Session, StoredPlan};
use crate::time::rfc3339_utc;

/// Dispatch one typed command against one session.
///
/// This is the single entry point a platform shell forwards into. It never
/// panics: malformed or impossible requests become [`Envelope::err`] values,
/// and no branch unwinds.
pub fn dispatch(session: &mut Session, command: Command) -> Envelope<Response> {
    match command {
        Command::StartScan(request) => handle_start_scan(session, &request),
        Command::CancelScan(request) => handle_cancel_scan(session, &request),
        Command::GetDriveSummary(request) => handle_get_drive_summary(session, &request),
        Command::GetCategories(request) => handle_get_categories(session, &request),
        Command::GetOpportunities(request) => handle_get_opportunities(session, &request),
        Command::PreviewPlan(request) => handle_preview_plan(session, &request),
        Command::ConfirmPlan(request) => handle_confirm_plan(session, &request),
        Command::ExecutePlan(request) => handle_execute_plan(session, &request),
        Command::GetHistory(request) => handle_get_history(session, &request),
        Command::ListDrives(_) => Envelope::ok(Response::Drives(
            session
                .drives_in_list_order()
                .into_iter()
                .cloned()
                .collect(),
        )),
    }
}

/// Parse and dispatch a raw JSON transport frame.
///
/// This is the convenience a Tauri shell can call with the raw invoke body.
/// Unknown command names yield `UNKNOWN_COMMAND`; malformed JSON or payloads
/// yield `INVALID_REQUEST`. Neither ever echoes a stack trace.
pub fn dispatch_json(session: &mut Session, request: &str) -> Envelope<Response> {
    let value: serde_json::Value = match serde_json::from_str(request) {
        Ok(value) => value,
        Err(error) => {
            return Envelope::err(
                ApiError::new(
                    codes::INVALID_REQUEST,
                    "The request is not valid JSON. Nothing was changed.",
                )
                .with_detail(format!("JSON parse error: {error}")),
            )
        }
    };
    dispatch_value(session, &value)
}

/// Dispatch an already-parsed JSON transport frame.
pub fn dispatch_value(session: &mut Session, request: &serde_json::Value) -> Envelope<Response> {
    match Command::from_wire(request) {
        Ok(command) => dispatch(session, command),
        Err(error) => Envelope::err(error),
    }
}

// ---------------------------------------------------------------------------
// Scanning: honestly unavailable in this build
// ---------------------------------------------------------------------------

fn handle_start_scan(session: &mut Session, request: &StartScanRequest) -> Envelope<Response> {
    if let Some(backend) = session.backend_mut() {
        return match backend.start_scan(request) {
            Ok(response) => Envelope::ok(Response::StartScan(response)),
            Err(error) => Envelope::err(error),
        };
    }
    Envelope::err(no_scan_backend("starting a scan"))
}

fn handle_cancel_scan(session: &mut Session, request: &CancelScanRequest) -> Envelope<Response> {
    if let Some(backend) = session.backend_mut() {
        return match backend.cancel_scan(request) {
            Ok(response) => Envelope::ok(Response::CancelScan(response)),
            Err(error) => Envelope::err(error),
        };
    }
    Envelope::err(no_scan_backend("cancelling a scan"))
}

fn no_scan_backend(action: &str) -> ApiError {
    ApiError::new(
        codes::SCAN_UNAVAILABLE,
        "No scan backend is attached to this build of the CoreSight IPC layer, \
         so scanning is unavailable. Nothing was started or cancelled.",
    )
    .with_detail(format!(
        "refused {action}: this crate contains no scanner and performs no filesystem access"
    ))
}

fn handle_get_history(_session: &mut Session, _request: &GetHistoryRequest) -> Envelope<Response> {
    Envelope::err(
        ApiError::new(
            codes::HISTORY_UNAVAILABLE,
            "No history store is attached to this build of the CoreSight IPC layer, \
             so snapshot history and deltas are unavailable.",
        )
        .with_detail(
            "coresight-ipc holds only the snapshots registered into this session; it does \
             not persist or load history",
        ),
    )
}

// ---------------------------------------------------------------------------
// Drives
// ---------------------------------------------------------------------------

fn handle_get_drive_summary(
    session: &Session,
    request: &GetDriveSummaryRequest,
) -> Envelope<Response> {
    let Some(drive) = session.drive(&request.drive_id) else {
        return Envelope::err(
            ApiError::new(
                codes::DRIVE_NOT_FOUND,
                "The requested drive is not known to this session.",
            )
            .with_detail(format!(
                "unknown drive id: {}",
                detail_token(&request.drive_id)
            )),
        );
    };
    let scans = session.scans_for_drive(&drive.id);
    if !drive.connected && scans.is_empty() {
        return Envelope::err(
            ApiError::new(
                codes::DRIVE_OFFLINE,
                "The requested drive is remembered but currently offline, and no \
                 retained snapshot is available to answer from.",
            )
            .with_detail(format!(
                "offline drive id: {}",
                detail_token(&request.drive_id)
            )),
        );
    }
    let last_scan = scans
        .iter()
        .rev()
        .find(|scan| scan.record.completed_at.is_some())
        .map(|scan| scan_summary_ref(scan));
    let totals = scans
        .iter()
        .rev()
        .find(|scan| scan.record.status.observes_full_scope())
        .map(|scan| scan_totals(scan));
    Envelope::ok(Response::DriveSummary(DriveSummary {
        drive_id: drive.id.clone(),
        connected: drive.connected,
        totals,
        // This build records no per-run exclusion report. `None` is the
        // honest "not recorded" value; an empty string would claim that
        // nothing was excluded.
        exclusions_note: None,
        last_scan,
        snapshot_count: u64::try_from(scans.len()).unwrap_or(u64::MAX),
    }))
}

fn scan_summary_ref(scan: &ScanState) -> ScanSummaryRef {
    ScanSummaryRef {
        scan_id: scan.record.scan_id.clone(),
        status: scan.record.status,
        completed_at: scan.record.completed_at.and_then(rfc3339_utc),
        complete_scope: scan.record.status.observes_full_scope(),
    }
}

fn scan_totals(scan: &ScanState) -> ScanTotals {
    ScanTotals {
        scan_id: scan.record.scan_id.clone(),
        bytes: scan.report.total_logical_size(),
        complete: report_is_complete(scan),
        scanned_entries: scan.report.scanned_entries,
        entries_without_size: scan.report.entries_without_size,
        errored_entries: scan.report.errored_entries,
        unclassified_entries: scan.report.unclassified_entries,
    }
}

// ---------------------------------------------------------------------------
// Categories
// ---------------------------------------------------------------------------

fn handle_get_categories(session: &Session, request: &GetCategoriesRequest) -> Envelope<Response> {
    let scan = match scan_for_serving(session, &request.scan_id) {
        Ok(scan) => scan,
        Err(error) => return Envelope::err(error),
    };
    let parent = match request.parent_category.as_deref() {
        None => None,
        Some(code) => match decode_category_code(code) {
            Some(category) => Some(category),
            None => {
                return Envelope::err(
                    ApiError::new(
                        codes::INVALID_REQUEST,
                        "`parentCategory` must be one of the stable category codes \
                         (for example `CACHE` or `DOWNLOADS`). Nothing was changed.",
                    )
                    .with_detail(format!("unknown parentCategory: {}", detail_token(code))),
                )
            }
        },
    };
    let categories = match parent {
        None => {
            let previous = session.previous_full_scope_scan(scan);
            top_level_categories(scan, previous)
        }
        Some(parent) => subcategory_categories(scan, parent),
    };
    Envelope::ok(Response::Categories(categories))
}

fn scan_for_serving<'a>(session: &'a Session, scan_id: &str) -> Result<&'a ScanState, ApiError> {
    let Some(scan) = session.scan(scan_id) else {
        return Err(ApiError::new(
            codes::SCAN_NOT_FOUND,
            "The requested scan is not available in this session. It may have been \
             cancelled, replaced, or never started.",
        )
        .with_detail(format!("unknown scan id: {}", detail_token(scan_id))));
    };
    match scan.record.status {
        ScanStatus::Completed | ScanStatus::CompletedWithLimits => Ok(scan),
        ScanStatus::Running => Err(ApiError::new(
            codes::SCAN_IN_PROGRESS,
            "The requested scan is still running. Categories and opportunities are \
             only served after a scan completes.",
        )
        .with_detail(format!("running scan id: {}", detail_token(scan_id)))),
        ScanStatus::Cancelled | ScanStatus::Failed => Err(ApiError::new(
            codes::SCAN_UNAVAILABLE,
            "The requested scan did not complete its declared scope, so its results \
             cannot be presented as a complete report.",
        )
        .with_detail(format!(
            "scan id {} has status {:?}",
            detail_token(scan_id),
            scan.record.status
        ))),
    }
}

fn report_is_complete(scan: &ScanState) -> bool {
    scan.report.entries_without_size == 0 && scan.report.errored_entries == 0
}

/// The 18 top-level categories, in `Category::ALL` order.
fn top_level_categories(scan: &ScanState, previous: Option<&ScanState>) -> Vec<CategoryResponse> {
    let total_bytes = scan.report.total_logical_size();
    let complete = report_is_complete(scan);
    // Rule: a share is unknown when the parent total is zero OR unknown.
    // Otherwise an empty category legitimately shows `0.0` (a true 0%).
    let denominator = if total_bytes == 0 || !complete {
        None
    } else {
        Some(total_bytes)
    };
    let previous_report = previous.map(|scan| &scan.report);
    let delta_comparable =
        previous.is_some() && complete && previous.map(report_is_complete).unwrap_or(false);

    Category::ALL
        .iter()
        .enumerate()
        .map(|(index, category)| {
            let totals = scan.report.category.get(index).copied().unwrap_or_default();
            let share = denominator.map(|total| totals.logical_size as f64 / total as f64);
            let delta = if delta_comparable {
                previous_report.map(|report| {
                    let old = report.category.get(index).copied().unwrap_or_default();
                    saturating_delta(totals.logical_size, old.logical_size)
                })
            } else {
                None
            };
            CategoryResponse {
                id: category.code().to_string(),
                name: category_title(*category).to_string(),
                bytes: totals.logical_size,
                share_of_parent: share,
                delta_since_last: delta,
                item_count: totals.entries,
                complete: totals.entries_without_size == 0 && scan.report.errored_entries == 0,
                entries_without_size: totals.entries_without_size,
            }
        })
        .collect()
}

/// One category's subcategory breakdown, derived from the stored classified
/// entries of that category.
///
/// Only buckets the scan actually contains are returned — the taxonomy has no
/// canonical subcategory order and inventing empty buckets would present
/// "zero" as if it were measured. Subcategories are ordered by code, with the
/// no-subcategory bucket last.
fn subcategory_categories(scan: &ScanState, parent: Category) -> Vec<CategoryResponse> {
    let mut buckets: std::collections::BTreeMap<Option<String>, SubcategoryBucket> =
        std::collections::BTreeMap::new();
    let mut parent_bytes: u64 = 0;
    let mut parent_unknown = false;

    for entry in &scan.record.snapshot.entries {
        let entry_category = entry
            .classification
            .as_ref()
            .and_then(|classification| decode_category_code(&classification.category))
            .unwrap_or(Category::Unknown);
        if entry_category != parent {
            continue;
        }
        let key = entry
            .classification
            .as_ref()
            .and_then(|classification| classification.subcategory.clone());
        let bucket = buckets.entry(key).or_default();
        bucket.entries = bucket.entries.saturating_add(1);
        match entry.size {
            Some(size) => {
                bucket.bytes = bucket.bytes.saturating_add(size);
                parent_bytes = parent_bytes.saturating_add(size);
            }
            None => {
                bucket.entries_without_size = bucket.entries_without_size.saturating_add(1);
                parent_unknown = true;
            }
        }
        if entry.observation_error.is_some() {
            bucket.errored = bucket.errored.saturating_add(1);
            parent_unknown = true;
        }
    }

    let denominator = if parent_bytes == 0 || parent_unknown {
        None
    } else {
        Some(parent_bytes)
    };

    let mut out: Vec<CategoryResponse> = Vec::with_capacity(buckets.len());
    for (key, bucket) in &buckets {
        let Some(code) = key else {
            continue;
        };
        out.push(subcategory_row(
            code,
            subcategory_name(Some(code.as_str())),
            bucket,
            denominator,
        ));
    }
    if let Some(bucket) = buckets.get(&None) {
        out.push(subcategory_row(
            "UNSUBCATEGORIZED",
            "Uncategorized",
            bucket,
            denominator,
        ));
    }
    out
}

#[derive(Default, Clone, Copy)]
struct SubcategoryBucket {
    entries: u64,
    bytes: u64,
    entries_without_size: u64,
    errored: u64,
}

fn subcategory_row(
    id: &str,
    name: &str,
    bucket: &SubcategoryBucket,
    denominator: Option<u64>,
) -> CategoryResponse {
    CategoryResponse {
        id: id.to_string(),
        name: name.to_string(),
        bytes: bucket.bytes,
        share_of_parent: denominator.map(|total| bucket.bytes as f64 / total as f64),
        // No per-subcategory baseline is retained in this build. Unknown is
        // `None` — never a fabricated zero.
        delta_since_last: None,
        item_count: bucket.entries,
        complete: bucket.entries_without_size == 0 && bucket.errored == 0,
        entries_without_size: bucket.entries_without_size,
    }
}

fn saturating_delta(new: u64, old: u64) -> i64 {
    let difference = (new as i128) - (old as i128);
    if difference > i64::MAX as i128 {
        i64::MAX
    } else if difference < i64::MIN as i128 {
        i64::MIN
    } else {
        difference as i64
    }
}

fn decode_category_code(code: &str) -> Option<Category> {
    Category::ALL
        .iter()
        .copied()
        .find(|category| category.code() == code)
}

fn category_title(category: Category) -> &'static str {
    match category {
        Category::Applications => "Applications",
        Category::ApplicationData => "Application data",
        Category::Games => "Games",
        Category::Documents => "Documents",
        Category::Images => "Images",
        Category::Video => "Video",
        Category::Audio => "Audio",
        Category::Downloads => "Downloads",
        Category::Archives => "Archives",
        Category::Development => "Development",
        Category::TemporaryData => "Temporary data",
        Category::Cache => "Cache",
        Category::Logs => "Logs",
        Category::Backups => "Backups",
        Category::SystemData => "System data",
        Category::UserData => "User data",
        Category::Other => "Other",
        Category::Unknown => "Unknown",
    }
}

fn subcategory_name(code: Option<&str>) -> &str {
    match code {
        Some("INSTALLER") => "Installer",
        Some("ARCHIVE") => "Archive",
        Some("DISK_IMAGE") => "Disk image",
        Some("DEPENDENCY_TREE") => "Dependency tree",
        Some("BUILD_OUTPUT") => "Build output",
        Some("VCS_INTERNALS") => "Version-control internals",
        Some("LOG_FILE") => "Log files",
        // A stored code this build does not know is shown verbatim rather
        // than renamed into a category it might not mean.
        Some(other) => other,
        None => "Uncategorized",
    }
}

// ---------------------------------------------------------------------------
// Opportunities
// ---------------------------------------------------------------------------

fn handle_get_opportunities(
    session: &Session,
    request: &GetOpportunitiesRequest,
) -> Envelope<Response> {
    let scan = match scan_for_serving(session, &request.scan_id) {
        Ok(scan) => scan,
        Err(error) => return Envelope::err(error),
    };
    Envelope::ok(Response::Opportunities(opportunities_for_scan(scan)))
}

fn opportunities_for_scan(scan: &ScanState) -> Vec<Opportunity> {
    scan.plan
        .items
        .iter()
        .filter_map(|item| {
            let tier = match item.tier {
                SafetyTier::Safe => OpportunityTier::Safe,
                SafetyTier::Review => OpportunityTier::Review,
                // The planner never emits OffLimits; if a future planner
                // did, it still must never become an actionable IPC card.
                SafetyTier::OffLimits => return None,
            };
            let totals = scan.report.totals_for(item.category);
            Some(Opportunity {
                id: opportunity_id_for(&scan.record.scan_id, item),
                tier,
                title: item.title.clone(),
                what_it_is: what_it_is(item, totals.entries, totals.entries_without_size),
                why_recommended: item.why_recommended.clone(),
                bytes: item.recoverable_bytes,
                recoverable_bytes: item.recoverable_bytes,
                stays_untouched: item.what_stays_untouched.clone(),
                consequence: item.consequence.clone(),
                preview: opportunity_preview(item),
            })
        })
        .collect()
}

fn what_it_is(item: &CleanupItem, entries: u64, entries_without_size: u64) -> String {
    let entry_word = if entries == 1 { "entry" } else { "entries" };
    let size_note = if entries_without_size == 0 {
        "every entry had a known size".to_string()
    } else {
        let missing_word = if entries_without_size == 1 {
            "entry"
        } else {
            "entries"
        };
        let verb = if entries_without_size == 1 {
            "is"
        } else {
            "are"
        };
        format!(
            "{entries_without_size} {missing_word} had no known size and {verb} excluded from \
             the byte figure"
        )
    };
    format!(
        "{entries} {entry_word} classified as {} in this scan; {size_note}.",
        item.title
    )
}

fn opportunity_preview(item: &CleanupItem) -> OpportunityPreview {
    let destination = item
        .destination
        .as_deref()
        .unwrap_or("an unspecified destination");
    let undo = item.undo.as_deref().unwrap_or("an unspecified undo path");
    OpportunityPreview {
        summary: format!(
            "Move {} bytes ({}) to {destination}; undo by {undo}.",
            item.recoverable_bytes, item.title
        ),
        bytes: item.recoverable_bytes,
        destination: item.destination.clone(),
        undo_path: item.undo.clone(),
    }
}

// ---------------------------------------------------------------------------
// Plan preview / confirm / execute
// ---------------------------------------------------------------------------

fn handle_preview_plan(session: &mut Session, request: &PreviewPlanRequest) -> Envelope<Response> {
    match build_stored_plan(session, request) {
        Ok(stored) => {
            let preview = stored.preview.clone();
            session.store_plan(stored);
            Envelope::ok(Response::PlanPreview(preview))
        }
        Err(error) => Envelope::err(error),
    }
}

fn build_stored_plan(
    session: &Session,
    request: &PreviewPlanRequest,
) -> Result<StoredPlan, ApiError> {
    if request.opportunity_ids.is_empty() {
        return Err(ApiError::new(
            codes::NOTHING_TO_DO,
            "No opportunities were selected, so there is nothing to preview.",
        ));
    }
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for id in &request.opportunity_ids {
        if !seen.insert(id.as_str()) {
            return Err(ApiError::new(
                codes::INVALID_REQUEST,
                "The request repeats an opportunity id, so the plan's byte accounting \
                 would be ambiguous. Nothing was changed.",
            )
            .with_detail(format!("duplicate opportunity id: {}", detail_token(id))));
        }
    }

    let (first_scan, _) = session.find_opportunity(&request.opportunity_ids[0])?;
    let scan_id = first_scan.record.scan_id.clone();
    let mut items: Vec<CleanupItem> = Vec::with_capacity(request.opportunity_ids.len());
    for id in &request.opportunity_ids {
        let (scan, item) = session.find_opportunity(id)?;
        if scan.record.scan_id != scan_id {
            return Err(ApiError::new(
                codes::INVALID_REQUEST,
                "The request selects opportunities from different scans. A plan must \
                 resolve against one snapshot. Nothing was changed.",
            )
            .with_detail(format!(
                "expected scan {}, got {}",
                detail_token(&scan_id),
                detail_token(&scan.record.scan_id)
            )));
        }
        items.push(item.clone());
    }

    let claimed_bytes = items.iter().fold(0u64, |accumulator, item| {
        accumulator.saturating_add(item.recoverable_bytes)
    });
    let plan_id = format!("plan:{}", request.opportunity_ids.join("+"));
    // The subset items carry the scan-scoped ids the caller used, so the
    // preview's `itemId` values match the opportunity ids exactly.
    for (item, requested_id) in items.iter_mut().zip(&request.opportunity_ids) {
        item.id = requested_id.clone();
    }
    let subset = CleanupPlan {
        id: plan_id.clone(),
        items,
        total_recoverable_bytes: claimed_bytes,
        partial: first_scan.plan.partial,
        off_limits_excluded: Vec::new(),
    };
    let resolved = resolve_plan(
        &subset,
        &first_scan.record.snapshot,
        &ResolveOptions::default(),
    )
    .map_err(unresolvable)?;

    let mut preview_items: Vec<PlanPreviewItem> = Vec::with_capacity(resolved.target_count);
    for resolved_item in &resolved.items {
        let title = subset
            .items
            .iter()
            .find(|item| item.id == resolved_item.item_id)
            .map(|item| item.title.clone())
            .unwrap_or_else(|| resolved_item.item_id.clone());
        for target in &resolved_item.targets {
            preview_items.push(PlanPreviewItem {
                item_id: resolved_item.item_id.clone(),
                title: title.clone(),
                path: encode_path(&target.path),
                kind: match target.kind {
                    TargetKind::File => PreviewTargetKind::File,
                    TargetKind::Directory => PreviewTargetKind::Directory,
                },
                bytes: target.bytes,
            });
        }
    }

    let (destination, destination_complete) = single_destination(&subset);
    let (undo_path, undo_complete) = single_undo_path(&subset);
    let mut warnings: Vec<String> = Vec::new();
    if subset.partial {
        warnings.push(format!(
            "The scan is incomplete: {} entries could not be fully read and {} entries \
             had no known size. Byte totals cover only measured entries.",
            first_scan.report.errored_entries, first_scan.report.entries_without_size
        ));
    }
    if resolved.unmapped_bytes > 0 {
        warnings.push(format!(
            "{} of {} planned bytes could not be pinned to concrete files or \
             directories and are not included in this preview.",
            resolved.unmapped_bytes, claimed_bytes
        ));
    }
    if resolved.unresolved_entries > 0 {
        warnings.push(format!(
            "{} entries could not be included as concrete targets (unreadable, \
             unsized, links, or unsupported kinds).",
            resolved.unresolved_entries
        ));
    }
    if destination.is_none() {
        warnings.push(
            "The destination is not specified consistently for every item; this \
             preview is incomplete."
                .to_string(),
        );
    }
    if undo_path.is_none() {
        warnings.push(
            "The undo path is not specified consistently for every item; this \
             preview is incomplete."
                .to_string(),
        );
    }
    let complete = !subset.partial
        && resolved.unmapped_bytes == 0
        && destination.is_some()
        && destination_complete
        && undo_path.is_some()
        && undo_complete;

    Ok(StoredPlan {
        preview: PlanPreview {
            plan_id,
            items: preview_items,
            bytes: resolved.total_bytes,
            claimed_bytes,
            destination,
            undo_path,
            warnings,
            complete,
        },
        plan: subset,
    })
}

fn unresolvable(error: ResolveError) -> ApiError {
    ApiError::new(
        codes::PLAN_UNRESOLVABLE,
        "The selected opportunities could not be resolved to concrete files or \
         directories, so no preview can be shown. Nothing was changed.",
    )
    .with_detail(error.to_string())
}

fn single_destination(plan: &CleanupPlan) -> (Option<String>, bool) {
    let mut distinct: BTreeSet<String> = BTreeSet::new();
    let mut every_item_has_one = true;
    for item in &plan.items {
        match &item.destination {
            Some(destination) => {
                distinct.insert(destination.clone());
            }
            None => every_item_has_one = false,
        }
    }
    if every_item_has_one && distinct.len() == 1 {
        (distinct.into_iter().next(), true)
    } else {
        (None, every_item_has_one && distinct.is_empty())
    }
}

fn single_undo_path(plan: &CleanupPlan) -> (Option<String>, bool) {
    let mut distinct: BTreeSet<String> = BTreeSet::new();
    let mut every_item_has_one = true;
    for item in &plan.items {
        match &item.undo {
            Some(undo) => {
                distinct.insert(undo.clone());
            }
            None => every_item_has_one = false,
        }
    }
    if every_item_has_one && distinct.len() == 1 {
        (distinct.into_iter().next(), true)
    } else {
        (None, every_item_has_one && distinct.is_empty())
    }
}

fn handle_confirm_plan(session: &mut Session, request: &ConfirmPlanRequest) -> Envelope<Response> {
    let Some(stored) = session.plan(&request.plan_id) else {
        return Envelope::err(plan_not_found(&request.plan_id));
    };
    let mut reasons: Vec<String> = Vec::new();
    for warning in &stored.preview.warnings {
        if !request.acked_warnings.iter().any(|acked| acked == warning) {
            reasons.push(format!("WARNINGS_NOT_ACKED: {warning}"));
        }
    }
    if !stored.preview.complete {
        reasons.push(
            "PREVIEW_INCOMPLETE: the preview does not pin every planned byte to a \
             concrete target"
                .to_string(),
        );
    }
    // The real validator, under the real current-build policy. Reversible
    // actions are not authorized, so this always yields reasons today. That
    // rejection is the safety contract working.
    let validation = validate_plan(&stored.plan);
    reasons.extend(validation.blocking_messages());

    let accepted = reasons.is_empty();
    let response = ConfirmPlanResponse {
        accepted,
        rejected: Vec::new(),
    };
    if accepted {
        session.mark_confirmed(&request.plan_id);
        Envelope::ok(Response::ConfirmPlan(response))
    } else {
        Envelope::err(ApiError::plan_rejected(&reasons))
    }
}

fn handle_execute_plan(session: &mut Session, request: &ExecutePlanRequest) -> Envelope<Response> {
    let Some(stored) = session.plan(&request.plan_id) else {
        return Envelope::err(plan_not_found(&request.plan_id));
    };
    let validation = validate_plan(&stored.plan);
    let reasons = validation.blocking_messages();
    if !reasons.is_empty() {
        // Exactly the same typed rejection as confirmPlan: the policy is
        // evaluated here too and cannot be bypassed by skipping confirm.
        return Envelope::err(ApiError::plan_rejected(&reasons));
    }
    if !session.is_confirmed(&request.plan_id) {
        return Envelope::err(
            ApiError::new(
                codes::PLAN_NOT_CONFIRMED,
                "The plan has not been accepted by confirmPlan in this session, so it \
                 cannot be executed.",
            )
            .with_detail(format!(
                "unconfirmed plan id: {}",
                detail_token(&request.plan_id)
            )),
        );
    }
    Envelope::err(
        ApiError::new(
            codes::NOT_IMPLEMENTED_IN_THIS_BUILD,
            "This build has no executor attached, so plan execution is unavailable. \
             Nothing was changed.",
        )
        .with_detail("coresight-ipc performs no filesystem writes and ships no executor"),
    )
}

fn plan_not_found(plan_id: &str) -> ApiError {
    ApiError::new(
        codes::PLAN_NOT_FOUND,
        "The requested plan is not available in this session. Plans are created by \
         previewPlan and must be confirmed in the same session.",
    )
    .with_detail(format!("unknown plan id: {}", detail_token(plan_id)))
}
