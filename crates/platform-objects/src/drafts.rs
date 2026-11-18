//! Record draft settings (ADR 0036 §9).
//!
//! A record draft is a member's unfinished input, kept in their browser or
//! device until they save it, discard it, sign out or it expires. Drafts
//! are not record changes and never reach the engine: no record version,
//! history, event, change-log entry, audit entry or sync push. The engine
//! keeps one thing about them, the workspace's expiry, as metadata: any
//! member reads it, `objects.manage` changes it, and a change is versioned,
//! audited, published as `platform.objects.draft-settings-updated.v1` and
//! moves the workspace metadata version.

use crate::events::Change;
use crate::service::{MutationContext, ObjectService};
use crate::ObjectError;
use chrono::{DateTime, SubsecRound, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Days a draft is kept after its last change when the workspace has not
/// chosen.
pub const DRAFT_EXPIRY_DAYS_DEFAULT: i32 = 7;
/// The shortest expiry an administrator may choose.
pub const DRAFT_EXPIRY_DAYS_MIN: i32 = 1;
/// The longest expiry; draft keys unused for this long are deleted too.
pub const DRAFT_EXPIRY_DAYS_MAX: i32 = 90;

/// A workspace's record draft settings. Version 0, without a change time,
/// is the default no administrator has changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DraftSettings {
    pub expiry_days: i32,
    pub version: i64,
    pub updated_at: Option<DateTime<Utc>>,
    pub updated_by: Option<Uuid>,
}

impl Default for DraftSettings {
    fn default() -> Self {
        Self {
            expiry_days: DRAFT_EXPIRY_DAYS_DEFAULT,
            version: 0,
            updated_at: None,
            updated_by: None,
        }
    }
}

/// A change to the draft settings, against the version the administrator
/// read (0 for the default).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateDraftSettings {
    pub expiry_days: i32,
    pub expected_version: i64,
}

impl ObjectService {
    /// The workspace's draft settings, or the default. Every member reads
    /// them: the shell needs the expiry to sweep the member's own drafts.
    pub async fn draft_settings(&self) -> Result<DraftSettings, ObjectError> {
        let mut tx = self.begin().await?;
        let settings = tx.draft_settings().await?.unwrap_or_default();
        tx.commit().await?;
        Ok(settings)
    }

    /// Sets the draft expiry. Needs `objects.manage`, which governs
    /// workspace-wide engine settings; values outside 1 to 90 days are
    /// refused with `drafts.expiry_invalid`, and a stale version with
    /// `records.version_conflict`.
    pub async fn update_draft_settings(
        &self,
        input: UpdateDraftSettings,
        context: MutationContext,
    ) -> Result<DraftSettings, ObjectError> {
        self.caller().require_manage()?;
        if !(DRAFT_EXPIRY_DAYS_MIN..=DRAFT_EXPIRY_DAYS_MAX).contains(&input.expiry_days) {
            return Err(ObjectError::invalid(
                "expiry_days",
                "drafts.expiry_invalid",
                format!("choose between {DRAFT_EXPIRY_DAYS_MIN} and {DRAFT_EXPIRY_DAYS_MAX} days"),
            ));
        }
        let mut tx = self.begin().await?;
        let previous = tx.draft_settings().await?.unwrap_or_default();
        if previous.version != input.expected_version {
            return Err(ObjectError::VersionConflict);
        }
        let settings = DraftSettings {
            expiry_days: input.expiry_days,
            version: previous.version + 1,
            // Microseconds, as both stores keep them.
            updated_at: Some(Utc::now().trunc_subsecs(6)),
            updated_by: Some(self.caller().user_id()),
        };
        if !tx
            .put_draft_settings(&settings, input.expected_version)
            .await?
        {
            return Err(ObjectError::VersionConflict);
        }
        self.record_change(
            tx.as_mut(),
            context,
            Change::DraftSettingsUpdated(&previous, &settings),
        )
        .await?;
        tx.commit().await?;
        Ok(settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_seven_days_at_version_zero() {
        let settings = DraftSettings::default();
        assert_eq!(settings.expiry_days, 7);
        assert_eq!(settings.version, 0);
        assert!(settings.updated_at.is_none());
    }

    #[test]
    fn changes_name_only_the_number_of_days() {
        let input: Result<UpdateDraftSettings, _> =
            serde_json::from_str(r#"{"expiry_days": 30, "expected_version": 0, "store": "x"}"#);
        assert!(input.is_err(), "unknown fields are refused");
    }
}
