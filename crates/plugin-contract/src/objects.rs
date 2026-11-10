//! Engine objects a plugin declares (ADR 0033 §2, ADR 0015 §3).
//!
//! `[[objects]]` in `plugin.toml` describes object and field definitions in
//! the plugin's own namespace, shaped after the object engine's inputs. The
//! declaration is inert data: the kernel creates or migrates the objects in
//! a workspace through the object engine when the plugin is enabled there,
//! with `origin = plugin` and the plugin as owner. Nothing here touches
//! storage.
//!
//! The API name of every declared object and field is `<plugin id>:<name>`;
//! the plugin id is the namespace. A reference (lookup or related-to) names
//! its target by API name: one of the plugin's own objects, or an object of
//! a plugin it declares a dependency on. A plugin cannot target `custom`
//! objects, which only exist in some workspaces.
//!
//! A related-to field may start with an empty or partial allowlist: the
//! workspace's administrators choose its targets once it is installed
//! (ADR 0033 §2). An object may also declare *related actions*
//! (`[[objects.related_actions]]`): a create quick action (ADR 0032) the
//! engine adds to every object that enters one of its related-to fields'
//! allowlists, making a record of this object with that field set to the
//! current record.
//!
//! An object may also declare how its records appear on the *Activity
//! timeline* of the records they point at (`[objects.timeline]`, ADR 0033
//! §4): through which of its lookup or related-to fields, ordered by which
//! date, and when a record counts as open. The kernel knows nothing of what
//! the records are; it serves every declared source the same way.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Objects per plugin.
pub const MAX_PLUGIN_OBJECTS: usize = 20;
/// Fields per declared object. Workspace administrators may add their own
/// `custom` fields on top, up to the engine's limit per object.
pub const MAX_PLUGIN_OBJECT_FIELDS: usize = 100;
/// Picklist options per field.
pub const MAX_PLUGIN_FIELD_OPTIONS: usize = 200;
/// Objects a related-to field may allow, as in the engine. A plugin's
/// related-to field may start with fewer, none included.
pub const MAX_PLUGIN_REFERENCE_OBJECTS: usize = 10;
/// Related actions per declared object: an object an allowlist names has at
/// most 10 quick actions anyway (ADR 0032 §2).
pub const MAX_PLUGIN_RELATED_ACTIONS: usize = 10;
/// Fields a related action's form asks for, and presets it applies, as for
/// quick actions.
pub const MAX_RELATED_ACTION_FIELDS: usize = 20;
pub const MAX_RELATED_ACTION_PRESETS: usize = 20;
/// The engine's limits on text lengths and decimal places.
pub const MAX_PLUGIN_TEXT_LENGTH: i32 = 255;
pub const MAX_PLUGIN_LONG_TEXT_LENGTH: i32 = 131_072;
pub const MAX_PLUGIN_FIELD_SCALE: i32 = 8;
const MAX_LABEL_CHARS: usize = 80;
const MAX_DESCRIPTION_CHARS: usize = 1000;
const MAX_NAME_BYTES: usize = 63;

/// One object a plugin declares.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ObjectDecl {
    /// Short name; the API name is `<plugin id>:<name>`.
    pub name: String,
    pub label: String,
    pub plural_label: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Label of the standard name field; "Name" when left out.
    #[serde(default)]
    pub name_label: Option<String>,
    /// Three characters, a lowercase letter first, then lowercase letters
    /// or digits (ADR 0015 §3). Fixed once installed: public ids carry it.
    pub key_prefix: String,
    /// The sharing model the object starts with in each workspace. The
    /// workspace owns it afterwards; upgrades never change it.
    #[serde(default)]
    pub sharing_model: Option<ObjectSharingDecl>,
    #[serde(default)]
    pub searchable: Option<bool>,
    #[serde(default)]
    pub track_history: Option<bool>,
    #[serde(default)]
    pub in_navigation: Option<bool>,
    #[serde(default)]
    pub fields: Vec<ObjectFieldDecl>,
    /// Create quick actions this object offers to the objects its related-to
    /// fields allow.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related_actions: Vec<RelatedActionDecl>,
    /// How this object's records appear on the Activity timeline of the
    /// records they point at (ADR 0033 §4). At most one per object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeline: Option<TimelineDecl>,
}

/// Fields a timeline declaration reaches the viewed record through.
pub const MAX_TIMELINE_THROUGH: usize = 5;

/// A timeline source (ADR 0033 §4): this object's records are listed on the
/// timeline of every record one of the `through` fields points at.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TimelineDecl {
    /// Short names of 1 to 5 of the object's lookup or related-to fields,
    /// in order of preference: an item names the first that matched.
    pub through: Vec<String>,
    /// Short name of the object's date or date-time field that orders the
    /// timeline. It must be indexed, as the timeline filters and sorts on it.
    pub date: String,
    /// When a record is open: listed under "Open and upcoming" rather than
    /// "Past". Without it, every record is past.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open: Option<TimelineOpenDecl>,
}

/// When a timeline record counts as open.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum TimelineOpenDecl {
    /// While one of the object's picklists has a value, such as a task's
    /// status being `open`. The picklist must be indexed.
    Field { field: String, equals: String },
    /// `"upcoming"`: while the timeline date is today or later (UTC).
    Keyword(TimelineOpenKeyword),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TimelineOpenKeyword {
    Upcoming,
}

/// A create quick action (ADR 0032) that every object entering the
/// allowlist of `field` receives: it makes a record of the declaring object
/// with `field` set to the current record. Once created it is an ordinary
/// workspace action that administrators may edit or delete; the engine
/// creates it only when an object enters the allowlist.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RelatedActionDecl {
    /// Short name of one of the object's related-to fields.
    pub field: String,
    /// The quick action's name on each object it is added to.
    pub name: String,
    pub label: String,
    /// The fields the form asks for, by API name: `name` or
    /// `<plugin id>:<field>`.
    pub fields: Vec<String>,
    /// Constant values applied on save, by field API name.
    #[serde(default)]
    pub presets: BTreeMap<String, Value>,
}

/// The organisation-wide default for an object's records.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ObjectSharingDecl {
    Private,
    Read,
    ReadWrite,
}

impl ObjectSharingDecl {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::Read => "read",
            Self::ReadWrite => "read_write",
        }
    }
}

/// One field of a declared object.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ObjectFieldDecl {
    /// Short name; the API name is `<plugin id>:<name>`.
    pub name: String,
    pub label: String,
    pub field_type: ObjectFieldType,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub unique: bool,
    #[serde(default)]
    pub indexed: bool,
    #[serde(default)]
    pub searchable: bool,
    #[serde(default)]
    pub track_history: bool,
    #[serde(default)]
    pub max_length: Option<i32>,
    #[serde(default)]
    pub scale: Option<i32>,
    /// Picklist and multi-select picklist values.
    #[serde(default)]
    pub options: Vec<ObjectFieldOption>,
    /// A lookup's target, by API name.
    #[serde(default)]
    pub reference_object: Option<String>,
    /// A related-to field's allowlist, by API name.
    #[serde(default)]
    pub reference_objects: Vec<String>,
    /// A lookup's delete behaviour; `clear` when left out.
    #[serde(default)]
    pub delete_behaviour: Option<ObjectDeleteBehaviour>,
}

/// The engine field types a plugin may declare. Formula, master-detail and
/// roll-up summary fields are left out for now: they tie objects together
/// in ways an upgrade could not migrate safely.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ObjectFieldType {
    Text,
    LongText,
    Email,
    Phone,
    Url,
    Number,
    Currency,
    Percent,
    Boolean,
    Date,
    DateTime,
    Picklist,
    MultiPicklist,
    Lookup,
    RelatedTo,
}

impl ObjectFieldType {
    /// The engine's name for the type.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::LongText => "long_text",
            Self::Email => "email",
            Self::Phone => "phone",
            Self::Url => "url",
            Self::Number => "number",
            Self::Currency => "currency",
            Self::Percent => "percent",
            Self::Boolean => "boolean",
            Self::Date => "date",
            Self::DateTime => "date_time",
            Self::Picklist => "picklist",
            Self::MultiPicklist => "multi_picklist",
            Self::Lookup => "lookup",
            Self::RelatedTo => "related_to",
        }
    }

    fn is_picklist(self) -> bool {
        matches!(self, Self::Picklist | Self::MultiPicklist)
    }

    fn is_numeric(self) -> bool {
        matches!(self, Self::Number | Self::Currency | Self::Percent)
    }

    fn may_be_unique(self) -> bool {
        matches!(
            self,
            Self::Text | Self::Email | Self::Phone | Self::Url | Self::Number
        )
    }

    fn is_textual(self) -> bool {
        matches!(
            self,
            Self::Text | Self::LongText | Self::Email | Self::Phone | Self::Url
        )
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ObjectDeleteBehaviour {
    Clear,
    Restrict,
}

impl ObjectDeleteBehaviour {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Clear => "clear",
            Self::Restrict => "restrict",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ObjectFieldOption {
    pub value: String,
    pub label: String,
    #[serde(default)]
    pub default: bool,
}

/// Why a declared object or field is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectDeclIssue {
    pub object: String,
    /// The field, when the issue is about one.
    pub field: Option<String>,
    pub reason: &'static str,
}

/// Whether `name` is a valid unqualified engine name (ADR 0015 §3): a
/// lowercase letter, then lowercase letters, digits or single underscores,
/// at most 63 characters, not ending in an underscore.
pub fn valid_object_name(name: &str) -> bool {
    (1..=MAX_NAME_BYTES).contains(&name.len())
        && name.as_bytes()[0].is_ascii_lowercase()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        && !name.ends_with('_')
        && !name.contains("__")
}

/// Whether a plugin id can be an engine namespace: dotted tokens that each
/// start with a lowercase letter and hold lowercase letters, digits or
/// underscores, and never `custom`.
pub fn valid_object_namespace(namespace: &str) -> bool {
    namespace != "custom"
        && (3..=100).contains(&namespace.len())
        && namespace.split('.').count() >= 2
        && namespace.split('.').all(|token| {
            token.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
                && token
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        })
}

/// Whether a key prefix is a plugin prefix: a lowercase letter, then two
/// lowercase letters or digits. Workspace objects take digit-first ones.
pub fn valid_plugin_key_prefix(prefix: &str) -> bool {
    let bytes = prefix.as_bytes();
    bytes.len() == 3
        && bytes[0].is_ascii_lowercase()
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

fn printable(text: &str, max: usize) -> bool {
    let text = text.trim();
    !text.is_empty() && text.chars().count() <= max && !text.chars().any(char::is_control)
}

/// Validates a plugin's object declarations. `dependencies` are the plugin
/// ids the manifest declares as dependencies.
pub fn validate_objects<'a>(
    plugin_id: &str,
    objects: &[ObjectDecl],
    dependencies: impl IntoIterator<Item = &'a str>,
) -> Result<(), ObjectDeclIssue> {
    if objects.is_empty() {
        return Ok(());
    }
    let dependencies: std::collections::BTreeSet<&str> = dependencies.into_iter().collect();
    let object_issue = |object: &ObjectDecl, reason| ObjectDeclIssue {
        object: object.name.clone(),
        field: None,
        reason,
    };
    if !valid_object_namespace(plugin_id) {
        return Err(object_issue(
            &objects[0],
            "the plugin id is not a valid object namespace: use dotted tokens of lowercase letters, digits and underscores",
        ));
    }
    if objects.len() > MAX_PLUGIN_OBJECTS {
        return Err(object_issue(
            &objects[0],
            "a plugin declares at most 20 objects",
        ));
    }
    let mut names = std::collections::BTreeSet::new();
    let mut prefixes = std::collections::BTreeSet::new();
    for object in objects {
        if !valid_object_name(&object.name) {
            return Err(object_issue(
                object,
                "name must be a lowercase letter followed by lowercase letters, digits or single underscores",
            ));
        }
        if !names.insert(object.name.as_str()) {
            return Err(object_issue(object, "duplicate object name"));
        }
        if !valid_plugin_key_prefix(&object.key_prefix) {
            return Err(object_issue(
                object,
                "key_prefix must be three characters: a lowercase letter, then lowercase letters or digits",
            ));
        }
        if !prefixes.insert(object.key_prefix.as_str()) {
            return Err(object_issue(object, "duplicate key prefix"));
        }
        if !printable(&object.label, MAX_LABEL_CHARS)
            || !printable(&object.plural_label, MAX_LABEL_CHARS)
            || object
                .name_label
                .as_deref()
                .is_some_and(|label| !printable(label, MAX_LABEL_CHARS))
        {
            return Err(object_issue(
                object,
                "labels must be 1 to 80 printable characters",
            ));
        }
        if object.description.as_deref().is_some_and(|description| {
            description.chars().count() > MAX_DESCRIPTION_CHARS
                || description
                    .chars()
                    .any(|character| character.is_control() && character != '\n')
        }) {
            return Err(object_issue(
                object,
                "description is too long or not printable",
            ));
        }
        if object.fields.len() > MAX_PLUGIN_OBJECT_FIELDS {
            return Err(object_issue(
                object,
                "an object declares at most 100 fields",
            ));
        }
    }
    for object in objects {
        let mut fields = std::collections::BTreeSet::new();
        for field in &object.fields {
            let issue = |reason| ObjectDeclIssue {
                object: object.name.clone(),
                field: Some(field.name.clone()),
                reason,
            };
            if !valid_object_name(&field.name) {
                return Err(issue(
                    "name must be a lowercase letter followed by lowercase letters, digits or single underscores",
                ));
            }
            if !fields.insert(field.name.as_str()) {
                return Err(issue("duplicate field name"));
            }
            validate_field(plugin_id, &names, &dependencies, field).map_err(issue)?;
        }
        if object.related_actions.len() > MAX_PLUGIN_RELATED_ACTIONS {
            return Err(object_issue(
                object,
                "an object declares at most 10 related actions",
            ));
        }
        let mut actions = std::collections::BTreeSet::new();
        for action in &object.related_actions {
            let issue = |reason| ObjectDeclIssue {
                object: object.name.clone(),
                field: Some(format!("related_actions/{}", action.name)),
                reason,
            };
            if !valid_object_name(&action.name) {
                return Err(issue(
                    "a related action's name must be a lowercase letter followed by lowercase letters, digits or single underscores",
                ));
            }
            if !actions.insert(action.name.as_str()) {
                return Err(issue("duplicate related action name"));
            }
            validate_related_action(plugin_id, object, action).map_err(issue)?;
        }
        if let Some(timeline) = &object.timeline {
            validate_timeline(object, timeline).map_err(|(part, reason)| ObjectDeclIssue {
                object: object.name.clone(),
                field: Some(format!("timeline/{part}")),
                reason,
            })?;
        }
    }
    Ok(())
}

/// Checks a timeline declaration against the object that makes it. The
/// error names the part at fault: `through`, `date` or `open`.
fn validate_timeline(
    object: &ObjectDecl,
    timeline: &TimelineDecl,
) -> Result<(), (&'static str, &'static str)> {
    let declared = |name: &str| object.fields.iter().find(|field| field.name == name);
    if timeline.through.is_empty() || timeline.through.len() > MAX_TIMELINE_THROUGH {
        return Err(("through", "a timeline is reached through 1 to 5 fields"));
    }
    let mut seen = std::collections::BTreeSet::new();
    for name in &timeline.through {
        if !seen.insert(name.as_str()) {
            return Err(("through", "a timeline names each through field once"));
        }
        match declared(name) {
            Some(field)
                if matches!(
                    field.field_type,
                    ObjectFieldType::Lookup | ObjectFieldType::RelatedTo
                ) => {}
            _ => {
                return Err((
                    "through",
                    "through must name the object's lookup or related-to fields",
                ))
            }
        }
    }
    match declared(&timeline.date) {
        Some(field)
            if matches!(
                field.field_type,
                ObjectFieldType::Date | ObjectFieldType::DateTime
            ) =>
        {
            if !field.indexed {
                return Err((
                    "date",
                    "the timeline date must be indexed: the timeline filters and sorts on it",
                ));
            }
        }
        _ => {
            return Err((
                "date",
                "date must name one of the object's date or date-time fields",
            ))
        }
    }
    match &timeline.open {
        None | Some(TimelineOpenDecl::Keyword(TimelineOpenKeyword::Upcoming)) => {}
        Some(TimelineOpenDecl::Field { field, equals }) => match declared(field) {
            Some(picklist) if picklist.field_type == ObjectFieldType::Picklist => {
                if !picklist
                    .options
                    .iter()
                    .any(|option| option.value == *equals)
                {
                    return Err(("open", "open.equals must be one of the picklist's options"));
                }
                if !picklist.indexed {
                    return Err((
                        "open",
                        "the open picklist must be indexed: the timeline filters on it",
                    ));
                }
            }
            _ => {
                return Err((
                    "open",
                    "open.field must name one of the object's picklist fields",
                ))
            }
        },
    }
    Ok(())
}

/// Checks a related action against the object that declares it: its field
/// is one of the object's related-to fields, its form asks for fields the
/// object has, and its presets are constants of the right type.
fn validate_related_action(
    plugin_id: &str,
    object: &ObjectDecl,
    action: &RelatedActionDecl,
) -> Result<(), &'static str> {
    if !printable(&action.label, MAX_LABEL_CHARS) {
        return Err("label must be 1 to 80 printable characters");
    }
    let declared = |name: &str| -> Option<&ObjectFieldDecl> {
        let short = name.strip_prefix(plugin_id)?.strip_prefix(':')?;
        object.fields.iter().find(|field| field.name == short)
    };
    match object
        .fields
        .iter()
        .find(|field| field.name == action.field)
    {
        Some(field) if field.field_type == ObjectFieldType::RelatedTo => {}
        _ => return Err("field must name one of the object's related-to fields"),
    }
    let target = format!("{plugin_id}:{}", action.field);
    if action.fields.is_empty() || action.fields.len() > MAX_RELATED_ACTION_FIELDS {
        return Err("a related action asks for 1 to 20 fields");
    }
    let mut seen = std::collections::BTreeSet::new();
    for name in &action.fields {
        if !seen.insert(name.as_str()) {
            return Err("a related action asks for each field once");
        }
        if *name == target {
            return Err("the related-to field is set to the current record, not asked for");
        }
        if name != "name" && declared(name).is_none() {
            return Err("fields must be `name` or API names of the object's declared fields");
        }
    }
    if action.presets.len() > MAX_RELATED_ACTION_PRESETS {
        return Err("a related action sets at most 20 presets");
    }
    for (name, value) in &action.presets {
        if *name == target {
            return Err("the related-to field is set to the current record, not preset");
        }
        let valid = if name == "name" {
            value
                .as_str()
                .is_some_and(|text| printable(text, MAX_PRESET_TEXT_CHARS))
        } else {
            match declared(name) {
                Some(field) => preset_fits(field, value),
                None => {
                    return Err(
                        "presets must name `name` or API names of the object's declared fields",
                    )
                }
            }
        };
        if !valid {
            return Err("a preset must be a constant of its field's type: an active option of a picklist, a number, true or false, or text");
        }
    }
    Ok(())
}

const MAX_PRESET_TEXT_CHARS: usize = 255;

/// Whether a preset is a constant the field accepts. The engine checks it
/// again, with its own value rules, when it adds the action to an object.
fn preset_fits(field: &ObjectFieldDecl, value: &Value) -> bool {
    let option = |text: &str| field.options.iter().any(|option| option.value == text);
    let date = |text: &str| {
        let bytes = text.as_bytes();
        bytes.len() == 10
            && bytes.iter().enumerate().all(|(index, byte)| match index {
                4 | 7 => *byte == b'-',
                _ => byte.is_ascii_digit(),
            })
            && matches!(text[5..7].parse::<u8>(), Ok(1..=12))
            && matches!(text[8..10].parse::<u8>(), Ok(1..=31))
    };
    match field.field_type {
        ObjectFieldType::Text
        | ObjectFieldType::LongText
        | ObjectFieldType::Email
        | ObjectFieldType::Phone
        | ObjectFieldType::Url => value.as_str().is_some_and(|text| {
            let limit = field
                .max_length
                .and_then(|length| usize::try_from(length).ok())
                .unwrap_or(MAX_PRESET_TEXT_CHARS);
            !text.trim().is_empty() && text.chars().count() <= limit
        }),
        ObjectFieldType::Number | ObjectFieldType::Currency | ObjectFieldType::Percent => {
            value.as_f64().is_some_and(f64::is_finite)
        }
        ObjectFieldType::Boolean => value.is_boolean(),
        ObjectFieldType::Date => value.as_str().is_some_and(date),
        ObjectFieldType::DateTime => value.as_str().is_some_and(|text| {
            text.len() > 19 && text.get(..10).is_some_and(date) && text.as_bytes()[10] == b'T'
        }),
        ObjectFieldType::Picklist => value.as_str().is_some_and(option),
        ObjectFieldType::MultiPicklist => value.as_array().is_some_and(|items| {
            !items.is_empty() && items.iter().all(|item| item.as_str().is_some_and(option))
        }),
        // A record id belongs to one workspace; a manifest cannot know it.
        ObjectFieldType::Lookup | ObjectFieldType::RelatedTo => false,
    }
}

fn validate_field(
    plugin_id: &str,
    own_objects: &std::collections::BTreeSet<&str>,
    dependencies: &std::collections::BTreeSet<&str>,
    field: &ObjectFieldDecl,
) -> Result<(), &'static str> {
    let kind = field.field_type;
    if !printable(&field.label, MAX_LABEL_CHARS) {
        return Err("label must be 1 to 80 printable characters");
    }
    if field.description.as_deref().is_some_and(|description| {
        description.chars().count() > MAX_DESCRIPTION_CHARS
            || description
                .chars()
                .any(|character| character.is_control() && character != '\n')
    }) {
        return Err("description is too long or not printable");
    }
    match (kind, field.max_length) {
        (ObjectFieldType::Text, Some(length))
            if !(1..=MAX_PLUGIN_TEXT_LENGTH).contains(&length) =>
        {
            return Err("max_length of a text field must be between 1 and 255");
        }
        (ObjectFieldType::LongText, Some(length))
            if !(1..=MAX_PLUGIN_LONG_TEXT_LENGTH).contains(&length) =>
        {
            return Err("max_length of a long text field must be between 1 and 131072");
        }
        (ObjectFieldType::Text | ObjectFieldType::LongText, _) | (_, None) => {}
        (_, Some(_)) => return Err("only text fields take max_length"),
    }
    match (kind.is_numeric(), field.scale) {
        (true, Some(scale)) if !(0..=MAX_PLUGIN_FIELD_SCALE).contains(&scale) => {
            return Err("scale must be between 0 and 8");
        }
        (false, Some(_)) => return Err("only number, currency and percent fields take scale"),
        _ => {}
    }
    if field.unique && !kind.may_be_unique() {
        return Err("only text, email, phone, URL and number fields can be unique");
    }
    if field.searchable && !kind.is_textual() {
        return Err("only text fields can be searchable");
    }
    if kind.is_picklist() {
        if field.options.is_empty() || field.options.len() > MAX_PLUGIN_FIELD_OPTIONS {
            return Err("picklist fields need 1 to 200 options");
        }
        let mut values = std::collections::BTreeSet::new();
        for option in &field.options {
            if !printable(&option.value, 255)
                || !printable(&option.label, MAX_LABEL_CHARS)
                || !values.insert(option.value.trim().to_lowercase())
            {
                return Err("options need distinct values and printable labels");
            }
        }
        if kind == ObjectFieldType::Picklist
            && field.options.iter().filter(|option| option.default).count() > 1
        {
            return Err("a picklist has at most one default option");
        }
    } else if !field.options.is_empty() {
        return Err("only picklist fields take options");
    }
    let target_allowed = |target: &str| -> bool {
        let Some((namespace, name)) = target.split_once(':') else {
            return false;
        };
        if !valid_object_name(name) {
            return false;
        }
        if namespace == plugin_id {
            own_objects.contains(name)
        } else {
            valid_object_namespace(namespace) && dependencies.contains(namespace)
        }
    };
    match (kind, field.reference_object.as_deref()) {
        (ObjectFieldType::Lookup, Some(target)) if target_allowed(target) => {}
        (ObjectFieldType::Lookup, _) => {
            return Err("a lookup needs a reference_object: one of this plugin's objects, or an object of a plugin it depends on");
        }
        (_, Some(_)) => return Err("only lookup fields take reference_object"),
        (_, None) => {}
    }
    if kind == ObjectFieldType::RelatedTo {
        // The workspace's administrators choose the rest (ADR 0033 §2).
        if field.reference_objects.len() > MAX_PLUGIN_REFERENCE_OBJECTS {
            return Err("a related-to field allows at most 10 objects");
        }
        let mut seen = std::collections::BTreeSet::new();
        for target in &field.reference_objects {
            if !target_allowed(target) {
                return Err("related-to targets must be this plugin's objects, or objects of plugins it depends on");
            }
            if !seen.insert(target.as_str()) {
                return Err("related-to targets must be distinct");
            }
        }
    } else if !field.reference_objects.is_empty() {
        return Err("only related-to fields take reference_objects");
    }
    if field.delete_behaviour.is_some() && kind != ObjectFieldType::Lookup {
        return Err("only lookup fields take delete_behaviour");
    }
    Ok(())
}
