mod candidates;
mod dep_info;
mod evidence;
mod format;
mod inputs;
mod record;
mod verify;

#[cfg(test)]
mod tests;

pub use format::{BUILD_RECEIPT_FILE, BuildReceiptReason, BuildReceiptStatus, BuildReceiptSummary};
pub use record::record_build_receipt;
pub use verify::check_build_receipt;

pub(crate) use candidates::collect_build_candidates;
pub(crate) use dep_info::read_artifact_dep_info;
pub(crate) use inputs::{snapshot_paths, verify_snapshots};
