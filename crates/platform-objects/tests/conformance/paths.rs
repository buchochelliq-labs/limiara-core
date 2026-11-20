//! Path fields and picklist outcomes on every backend (ADR 0037 §1, §2):
//! options keep their outcome and weight, invalid ones are refused, an
//! object names one of its single-value picklists as its path field, the
//! field cannot be deleted while it is the path, and replica definitions
//! carry it.

use super::{caller, codes, context, field, object, option, service, ADMIN_PERMISSIONS};
use platform_objects::replica_definitions::RemoteObject;
use platform_objects::*;
use std::sync::Arc;
use uuid::Uuid;

const DEAL: &str = "custom:deal";
const STAGE: &str = "custom:stage";

fn update(version: i64, path_field: Option<Option<&str>>) -> UpdateObject {
    UpdateObject {
        path_field: path_field.map(|field| field.map(str::to_owned)),
        expected_version: version,
        label: None,
        plural_label: None,
        description: None,
        name_label: None,
        sharing_model: None,
        grant_hierarchy: None,
        track_history: None,
        searchable: None,
        in_navigation: None,
    }
}

fn stage(value: &str, outcome: OptionOutcome, weight: Option<u8>) -> PicklistOption {
    PicklistOption {
        outcome,
        weight,
        ..option(value, false)
    }
}

pub async fn run(store: Arc<dyn ObjectStore>, tenant: Uuid) {
    let admin = service(
        &store,
        caller(tenant, Uuid::now_v7(), &ADMIN_PERMISSIONS, &[]),
    );
    admin
        .create_object(object("deal", "Deal"), context())
        .await
        .expect("deal object");

    // --- Outcomes and weights on options -----------------------------------
    let mut picklist = field("stage", FieldType::Picklist);
    picklist.options = vec![
        stage("Prospecting", OptionOutcome::Open, Some(10)),
        stage("Proposal", OptionOutcome::Open, Some(60)),
        stage("Won", OptionOutcome::Positive, Some(100)),
        stage("Lost", OptionOutcome::Negative, Some(0)),
        // No weight: it is optional.
        stage("Parked", OptionOutcome::Open, None),
    ];
    admin
        .create_field(DEAL, picklist, context())
        .await
        .expect("stage with outcomes and weights");
    let (deal, _) = admin.describe(DEAL).await.expect("describe deal");
    let options = &deal.field(STAGE).expect("stage field").options;
    let read: Vec<(&str, OptionOutcome, Option<u8>)> = options
        .iter()
        .map(|option| (option.value.as_str(), option.outcome, option.weight))
        .collect();
    assert_eq!(
        read,
        vec![
            ("Prospecting", OptionOutcome::Open, Some(10)),
            ("Proposal", OptionOutcome::Open, Some(60)),
            ("Won", OptionOutcome::Positive, Some(100)),
            ("Lost", OptionOutcome::Negative, Some(0)),
            ("Parked", OptionOutcome::Open, None),
        ],
        "outcomes and weights are stored as given"
    );

    // A weight is a percentage.
    let mut heavy = field("confidence", FieldType::Picklist);
    heavy.options = vec![stage("Sure", OptionOutcome::Open, Some(101))];
    let refused = admin
        .create_field(DEAL, heavy, context())
        .await
        .expect_err("a weight over 100");
    assert!(codes(&refused).contains(&"objects.options_invalid"));
    // A multi-select picklist's values cannot close a record.
    let mut tags = field("tags", FieldType::MultiPicklist);
    tags.options = vec![stage("Done", OptionOutcome::Positive, None)];
    let refused = admin
        .create_field(DEAL, tags, context())
        .await
        .expect_err("an outcome on a multi-select picklist");
    assert!(codes(&refused).contains(&"objects.options_invalid"));
    admin
        .create_field(DEAL, field("next_step", FieldType::Text), context())
        .await
        .expect("a text field");

    // --- The path field ----------------------------------------------------
    assert_eq!(deal.path_field, None, "a new object has no path");
    for wrong in ["custom:next_step", "custom:missing"] {
        let (deal, _) = admin.describe(DEAL).await.expect("describe deal");
        let refused = admin
            .update_object(DEAL, update(deal.version, Some(Some(wrong))), context())
            .await
            .expect_err("a path field that is not a picklist of the object");
        assert!(
            codes(&refused).contains(&"objects.path_field_invalid"),
            "{wrong}"
        );
    }
    let (deal, _) = admin.describe(DEAL).await.expect("describe deal");
    let deal = admin
        .update_object(DEAL, update(deal.version, Some(Some(STAGE))), context())
        .await
        .expect("the stage as the path");
    assert_eq!(deal.path_field.as_deref(), Some(STAGE));
    let (described, _) = admin.describe(DEAL).await.expect("describe deal");
    assert_eq!(described.path_field.as_deref(), Some(STAGE), "stored");

    // Leaving it out of an update keeps it.
    let deal = admin
        .update_object(DEAL, update(deal.version, None), context())
        .await
        .expect("an update that leaves the path out");
    assert_eq!(deal.path_field.as_deref(), Some(STAGE));

    // Replica definitions carry it, so the device shows the same path.
    let remote = RemoteObject::from_definition(&described, AccessFlags::default(), None);
    assert_eq!(remote.path_field.as_deref(), Some(STAGE));

    // The path keeps its field.
    let stage_field = described.field(STAGE).expect("stage field").clone();
    assert!(matches!(
        admin
            .delete_field(DEAL, STAGE, stage_field.version, context())
            .await,
        Err(ObjectError::Conflict("objects.field_in_use"))
    ));

    // `null` clears it, and the field can then go.
    let deal = admin
        .update_object(DEAL, update(deal.version, Some(None)), context())
        .await
        .expect("no path");
    assert_eq!(deal.path_field, None);
    admin
        .delete_field(DEAL, STAGE, stage_field.version, context())
        .await
        .expect("a picklist that is no longer the path");
}
