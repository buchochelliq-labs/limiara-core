use super::check::{Catalog, Purpose};
use super::*;
use crate::{DefinitionStatus, FieldDefinition, ObjectDefinition, Origin, SharingModel};
use serde_json::json;
use std::collections::BTreeMap;
use uuid::Uuid;

fn field(object: &ObjectDefinition, api: &str, kind: FieldType) -> FieldDefinition {
    FieldDefinition {
        id: Uuid::now_v7(),
        object_id: object.id,
        api_name: api.into(),
        label: api.into(),
        description: None,
        field_type: kind,
        origin: Origin::Workspace,
        required: false,
        unique: false,
        indexed: false,
        searchable: false,
        track_history: false,
        max_length: None,
        scale: None,
        options: Vec::new(),
        default_value: None,
        reference_object_id: None,
        reference_object: None,
        reference_object_ids: Vec::new(),
        reference_objects: Vec::new(),
        delete_behaviour: None,
        formula: None,
        formula_type: None,
        rollup: None,
        lookup_filter: None,
        position: 0,
        status: DefinitionStatus::Active,
        version: 1,
        created_at: Utc::now(),
        created_by: None,
        updated_at: Utc::now(),
        updated_by: None,
    }
}

fn object(api: &str) -> ObjectDefinition {
    ObjectDefinition {
        id: Uuid::now_v7(),
        api_name: api.into(),
        label: api.into(),
        plural_label: api.into(),
        description: None,
        origin: Origin::Workspace,
        owner_plugin: None,
        key_prefix: "0a1".into(),
        name_label: "Name".into(),
        sharing_model: SharingModel::ReadWrite,
        grant_hierarchy: true,
        track_history: false,
        searchable: true,
        in_navigation: true,
        status: DefinitionStatus::Active,
        suspended: false,
        version: 1,
        created_at: Utc::now(),
        created_by: None,
        updated_at: Utc::now(),
        updated_by: None,
        fields: Vec::new(),
    }
}

struct Fixture {
    deal: ObjectDefinition,
    objects: BTreeMap<Uuid, ObjectDefinition>,
}

fn fixture() -> Fixture {
    let mut account = object("custom:account");
    account.fields = vec![field(&account, "custom:region", FieldType::Picklist)];
    let mut deal = object("custom:deal");
    let mut lookup = field(&deal, "custom:account", FieldType::Lookup);
    lookup.reference_object_id = Some(account.id);
    deal.fields = vec![
        field(&deal, "custom:amount", FieldType::Currency),
        field(&deal, "custom:stage", FieldType::Picklist),
        field(&deal, "custom:close", FieldType::Date),
        field(&deal, "custom:won", FieldType::Boolean),
        field(&deal, "custom:tags", FieldType::MultiPicklist),
        field(&deal, "custom:notes", FieldType::LongText),
        lookup,
    ];
    let objects = BTreeMap::from([(account.id, account)]);
    Fixture { deal, objects }
}

fn compile_as(
    fixture: &Fixture,
    text: &str,
    purpose: Purpose,
) -> Result<Formula, parse::FormulaError> {
    let catalog = Catalog::new(&fixture.deal, &fixture.objects, purpose);
    compile(text, &catalog, None)
}

/// Values by path text: `custom:amount`, `custom:account.custom:region`.
struct TestEnv {
    values: BTreeMap<String, Value>,
    prior: BTreeMap<String, Value>,
    new: bool,
}

fn key(field: &FieldRef) -> String {
    let leaf = match &field.leaf {
        Leaf::Standard(standard) => standard.as_str().to_owned(),
        Leaf::Field { api_name, .. } => api_name.clone(),
    };
    field
        .hops
        .iter()
        .cloned()
        .chain([leaf])
        .collect::<Vec<_>>()
        .join(".")
}

impl Env for TestEnv {
    fn value(&self, field: &FieldRef) -> Value {
        self.values.get(&key(field)).cloned().unwrap_or(Value::Null)
    }
    fn prior(&self, field: &FieldRef) -> Value {
        self.prior.get(&key(field)).cloned().unwrap_or(Value::Null)
    }
    fn is_new(&self) -> bool {
        self.new
    }
    fn now(&self) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-12T15:30:00Z")
            .expect("time")
            .with_timezone(&Utc)
    }
}

fn env() -> TestEnv {
    let values = BTreeMap::from([
        ("name".to_owned(), Value::Text("Big deal".into())),
        ("custom:amount".to_owned(), Value::Number(1250.5)),
        ("custom:stage".to_owned(), Value::Text("won".into())),
        (
            "custom:close".to_owned(),
            Value::Date(NaiveDate::from_ymd_opt(2026, 1, 31).expect("date")),
        ),
        ("custom:won".to_owned(), Value::Bool(true)),
        (
            "custom:tags".to_owned(),
            Value::Multi(vec!["a".into(), "b".into()]),
        ),
        (
            "custom:account.custom:region".to_owned(),
            Value::Text("emea".into()),
        ),
        (
            "custom:account.id".to_owned(),
            Value::Text("0a1000000000000001".into()),
        ),
    ]);
    let mut prior = values.clone();
    prior.insert("custom:stage".into(), Value::Text("open".into()));
    TestEnv {
        values,
        prior,
        new: false,
    }
}

fn run(text: &str) -> Result<Value, EvalError> {
    let fixture = fixture();
    let formula = compile_as(&fixture, text, Purpose::Save)
        .unwrap_or_else(|error| panic!("{text}: {error:?}"));
    evaluate(&formula, &env())
}

fn date(text: &str) -> Value {
    Value::Date(NaiveDate::parse_from_str(text, "%Y-%m-%d").expect("date"))
}

#[test]
fn evaluates_operators_functions_and_fields() {
    for (text, expected) in [
        ("1 + 2 * 3", Value::Number(7.0)),
        ("custom:amount / 2 > 600", Value::Bool(true)),
        (
            "'Deal: ' & name & ' ' & TEXT(custom:amount)",
            Value::Text("Deal: Big deal 1250.5".into()),
        ),
        (
            "ISPICKVAL(custom:stage, 'won') && custom:won",
            Value::Bool(true),
        ),
        ("custom:stage = 'Won'", Value::Bool(false)),
        (
            "IF(custom:amount > 1000, 'large', 'small')",
            Value::Text("large".into()),
        ),
        (
            "CASE(custom:stage, 'open', 1, 'won', 2, 0)",
            Value::Number(2.0),
        ),
        ("CASE(custom:stage, 'lost', 1, 9)", Value::Number(9.0)),
        ("INCLUDES(custom:tags, 'b')", Value::Bool(true)),
        ("ISBLANK(custom:notes) && ISBLANK('')", Value::Bool(true)),
        (
            "BLANKVALUE(custom:notes, 'none')",
            Value::Text("none".into()),
        ),
        ("LEN(name) + LEN(custom:notes)", Value::Number(8.0)),
        (
            "UPPER(LEFT(name, 3)) & RIGHT(name, 4) & MID(name, 5, 2)",
            Value::Text("BIGdealde".into()),
        ),
        (
            "CONTAINS(name, 'g d') && BEGINS(name, 'Big') && !BEGINS(custom:notes, '')",
            Value::Bool(true),
        ),
        (
            "SUBSTITUTE(name, 'g', 'gg')",
            Value::Text("Bigg deal".into()),
        ),
        ("ROUND(custom:amount / 3, 2)", Value::Number(416.83)),
        (
            "MOD(7, 3) + ABS(-2) + FLOOR(1.7) + CEILING(1.2)",
            Value::Number(6.0),
        ),
        (
            "MAX(1, custom:amount, 3) - MIN(5, 2)",
            Value::Number(1248.5),
        ),
        ("VALUE('12.5') * 2", Value::Number(25.0)),
        ("custom:close + 1", date("2026-02-01")),
        ("ADDMONTHS(custom:close, 1)", date("2026-02-28")),
        ("TODAY() - custom:close", Value::Number(254.0)),
        (
            "YEAR(TODAY()) * 10000 + MONTH(TODAY()) * 100 + DAY(TODAY())",
            Value::Number(20261012.0),
        ),
        ("DATE(2026, 2, 29)", Value::Null),
        ("DATEVALUE(NOW()) = TODAY()", Value::Bool(true)),
        ("custom:account.custom:region = 'emea'", Value::Bool(true)),
        (
            "ISCHANGED(custom:stage) && PRIORVALUE(custom:stage) = 'open'",
            Value::Bool(true),
        ),
        ("ISCHANGED(custom:amount)", Value::Bool(false)),
        ("ISNEW()", Value::Bool(false)),
        (
            "custom:amount - PRIORVALUE(custom:amount)",
            Value::Number(0.0),
        ),
        ("custom:notes > 'a'", Value::Null),
        ("IF(custom:notes > 'a', 1, 2)", Value::Number(2.0)),
        ("custom:notes = NULL", Value::Bool(true)),
    ] {
        match (text, run(text)) {
            ("DATE(2026, 2, 29)", Err(EvalError(message))) => {
                assert_eq!(message, "an invalid date")
            }
            (_, Ok(value)) => assert_eq!(value, expected, "{text}"),
            (_, Err(error)) => panic!("{text}: {error:?}"),
        }
    }
}

#[test]
fn runtime_errors_are_errors_not_values() {
    for text in [
        "1 / 0",
        "MOD(1, 0)",
        "VALUE('ten')",
        "DATE(2026, 13, 1)",
        "DATEVALUE('soon')",
    ] {
        assert!(run(text).is_err(), "{text}");
    }
    let doubling = format!(
        "SUBSTITUTE('{}', 'a', '{}')",
        "a".repeat(1000),
        "b".repeat(200)
    );
    assert!(
        run(&doubling).is_err(),
        "substitution cannot grow past the text limit"
    );
}

#[test]
fn checking_rejects_misuse_with_positions() {
    let fixture = fixture();
    for (text, purpose, fragment) in [
        (
            "custom:missing > 1",
            Purpose::Save,
            "has no field custom:missing",
        ),
        ("custom:amount.name", Purpose::Save, "is not a lookup"),
        ("custom:amount & 1", Purpose::Save, "cannot combine"),
        ("custom:stage > 1", Purpose::Save, "cannot combine"),
        (
            "IF(custom:amount, 1, 2)",
            Purpose::Save,
            "IF needs true or false",
        ),
        (
            "IF(custom:won, 1, 'x')",
            Purpose::Save,
            "values of one type",
        ),
        ("NOPE(1)", Purpose::Save, "is not a function"),
        ("LEN()", Purpose::Save, "LEN takes 1 arguments, not 0"),
        ("CASE(1, 2, 3)", Purpose::Save, "at least 4"),
        (
            "ISCHANGED(custom:amount)",
            Purpose::Field,
            "only be used in validation rules and automations",
        ),
        (
            "ISCHANGED(custom:account.name)",
            Purpose::Save,
            "not of a related record",
        ),
        ("custom:tags = 'a'", Purpose::Save, "cannot combine"),
        ("a.b.c.d.e", Purpose::Save, "at most 3 lookups"),
        (
            "PRIORVALUE(name & '')",
            Purpose::Save,
            "needs a field of this record",
        ),
    ] {
        let error = compile_as(&fixture, text, purpose).expect_err(text);
        assert!(
            error.message.contains(fragment),
            "{text}: {}",
            error.message
        );
        assert!(error.position >= 1);
    }
    let catalog = Catalog::new(&fixture.deal, &fixture.objects, Purpose::Save);
    assert!(compile("custom:amount", &catalog, Some(Type::Boolean))
        .expect_err("not a condition")
        .message
        .contains("must give true or false"));
}

#[test]
fn hop_chains_list_every_prefix_once() {
    let fixture = fixture();
    let formula = compile_as(
        &fixture,
        "custom:account.custom:region = 'x' || ISBLANK(custom:account.name)",
        Purpose::Save,
    )
    .expect("compiles");
    assert_eq!(
        formula.hop_chains(),
        vec![vec!["custom:account".to_owned()]]
    );
}

#[test]
fn record_values_follow_field_types() {
    let record = Record {
        id: Uuid::now_v7(),
        object_id: Uuid::now_v7(),
        public_id: "0a1000000000000001".into(),
        name: "Deal".into(),
        owner_id: None,
        record_type_id: None,
        values: BTreeMap::from([
            ("custom:amount".into(), json!("12.50")),
            ("custom:tags".into(), json!(["a"])),
            ("custom:close".into(), json!("2026-10-12")),
        ]),
        version: 3,
        created_at: Utc::now(),
        created_by: None,
        updated_at: Utc::now(),
        updated_by: None,
        touched_at: Utc::now(),
        archived_at: None,
        deleted_at: None,
    };
    let leaf = |api: &str, field_type| Leaf::Field {
        api_name: api.into(),
        field_type,
    };
    assert_eq!(
        record_value(&record, &leaf("custom:amount", FieldType::Currency)),
        Value::Number(12.5)
    );
    assert_eq!(
        record_value(&record, &leaf("custom:tags", FieldType::MultiPicklist)),
        Value::Multi(vec!["a".into()])
    );
    assert_eq!(
        record_value(&record, &leaf("custom:close", FieldType::Date)),
        date("2026-10-12")
    );
    assert_eq!(
        record_value(&record, &leaf("custom:none", FieldType::Text)),
        Value::Null
    );
    assert_eq!(
        record_value(&record, &Leaf::Standard(Standard::Version)),
        Value::Number(3.0)
    );
    assert_eq!(
        record_value(&record, &Leaf::Standard(Standard::Owner)),
        Value::Null
    );
    assert_eq!(
        Value::Date(NaiveDate::from_ymd_opt(2026, 1, 2).expect("date")).to_json(),
        json!("2026-01-02")
    );
}

fn formula_field(
    object: &ObjectDefinition,
    api: &str,
    text: &str,
    result: FieldType,
) -> FieldDefinition {
    let mut definition = field(object, api, FieldType::Formula);
    definition.formula = Some(text.into());
    definition.formula_type = Some(result);
    definition
}

/// The fixture with formula fields on both objects.
fn with_formulas(extra: &[(&str, &str, FieldType)]) -> Fixture {
    let mut fixture = fixture();
    let account_id = *fixture.objects.keys().next().expect("account");
    let account = fixture.objects.get_mut(&account_id).expect("account");
    let region = formula_field(
        account,
        "custom:region_label",
        "UPPER(TEXT(custom:region))",
        FieldType::Text,
    );
    account.fields.push(region);
    let deal = fixture.deal.clone();
    fixture.deal.fields.push(formula_field(
        &deal,
        "custom:net",
        "custom:amount * 0.9",
        FieldType::Currency,
    ));
    fixture.deal.fields.push(formula_field(
        &deal,
        "custom:label",
        "name & ' ' & TEXT(custom:net)",
        FieldType::Text,
    ));
    for (api, text, result) in extra {
        fixture
            .deal
            .fields
            .push(formula_field(&deal, api, text, *result));
    }
    fixture
}

#[test]
fn formulas_use_other_formula_fields_by_inlining_them() {
    let fixture = with_formulas(&[]);
    let check = |text: &str| {
        let formula = compile_as(&fixture, text, Purpose::Save)
            .unwrap_or_else(|error| panic!("{text}: {error:?}"));
        evaluate(&formula, &env()).expect("value")
    };
    assert_eq!(
        check("custom:label"),
        Value::Text("Big deal 1125.45".into())
    );
    assert_eq!(check("custom:net > 1000"), Value::Bool(true));
    // Through a lookup, the inlined formula reads the related record.
    let formula = compile_as(
        &fixture,
        "custom:account.custom:region_label",
        Purpose::Field,
    )
    .expect("hop");
    assert_eq!(
        formula.hop_chains(),
        vec![vec!["custom:account".to_owned()]]
    );
    assert_eq!(evaluate(&formula, &env()), Ok(Value::Text("EMEA".into())));
    let error = compile_as(&fixture, "ISCHANGED(custom:net)", Purpose::Save).expect_err("stored");
    assert!(error.message.contains("needs a stored field"), "{error:?}");
}

#[test]
fn formula_cycles_depth_and_size_are_refused() {
    let fixture = with_formulas(&[
        ("custom:a", "custom:b + 1", FieldType::Number),
        ("custom:b", "custom:a + 1", FieldType::Number),
    ]);
    let error = compile_as(&fixture, "custom:a", Purpose::Field).expect_err("cycle");
    assert!(error.message.contains("circle"), "{error:?}");
    // Editing a formula so that it comes back to itself is a cycle at once.
    let label = fixture.deal.field("custom:net").expect("net").id;
    let catalog = Catalog::editing(&fixture.deal, &fixture.objects, label);
    let error = compile("custom:label", &catalog, None).expect_err("edit cycle");
    assert!(error.message.contains("circle"), "{error:?}");

    let deep: Vec<(String, String)> = (1..=11)
        .map(|level| {
            (
                format!("custom:d{level}"),
                format!("custom:d{} + 1", level - 1),
            )
        })
        .collect();
    let mut extra: Vec<(&str, &str, FieldType)> = vec![("custom:d0", "1", FieldType::Number)];
    extra.extend(
        deep.iter()
            .map(|(api, text)| (api.as_str(), text.as_str(), FieldType::Number)),
    );
    let fixture = with_formulas(&extra);
    assert!(compile_as(&fixture, "custom:d9", Purpose::Field).is_ok());
    let error = compile_as(&fixture, "custom:d11", Purpose::Field).expect_err("deep");
    assert!(error.message.contains("levels deep"), "{error:?}");

    // Each level uses the one below three times, so the expansion grows
    // past the budget within the depth limit: 1, 5, 17, ... 4373 nodes.
    let wide: Vec<(String, String)> = (1..=7)
        .map(|level| {
            (
                format!("custom:w{level}"),
                format!("custom:w{0} + custom:w{0} + custom:w{0}", level - 1),
            )
        })
        .collect();
    let mut extra: Vec<(&str, &str, FieldType)> =
        vec![("custom:w0", "custom:amount", FieldType::Number)];
    extra.extend(
        wide.iter()
            .map(|(api, text)| (api.as_str(), text.as_str(), FieldType::Number)),
    );
    let fixture = with_formulas(&extra);
    // Nested formulas are charged once: 1,457 nodes fit the budget.
    compile_as(&fixture, "custom:w6", Purpose::Field).expect("within the budget");
    let error = compile_as(&fixture, "custom:w7", Purpose::Field).expect_err("wide");
    assert!(error.message.contains("too large"), "{error:?}");
}

#[test]
fn evaluates_text_number_and_date_functions() {
    let time = |text: &str| {
        Value::DateTime(
            DateTime::parse_from_rfc3339(text)
                .expect("time")
                .with_timezone(&Utc),
        )
    };
    for (text, expected) in [
        ("REGEX(name, '[A-Z][a-z]+ deal')", Value::Bool(true)),
        ("REGEX('abc', 'b')", Value::Bool(false)),
        ("REGEX(custom:notes, '')", Value::Bool(true)),
        ("ISNUMBER('1.5') && !ISNUMBER('x')", Value::Bool(true)),
        ("FIND('deal', name)", Value::Number(5.0)),
        ("FIND('x', name)", Value::Number(0.0)),
        ("FIND('e', 'eee', 2)", Value::Number(2.0)),
        ("LPAD('7', 3, '0')", Value::Text("007".into())),
        ("RPAD('ab', 4)", Value::Text("ab  ".into())),
        ("LPAD('abcdef', 3)", Value::Text("abc".into())),
        ("SQRT(16) + LOG(100)", Value::Number(6.0)),
        ("ROUND(LN(EXP(1)), 6)", Value::Number(1.0)),
        ("TRUNC(1.789, 2)", Value::Number(1.78)),
        ("TRUNC(-1.5)", Value::Number(-1.0)),
        ("WEEKDAY(DATE(2026, 10, 11))", Value::Number(1.0)),
        (
            "DATETIMEVALUE('2026-10-12 08:00:00')",
            time("2026-10-12T08:00:00Z"),
        ),
        (
            "DATETIMEVALUE(DATE(2026, 10, 12))",
            time("2026-10-12T00:00:00Z"),
        ),
    ] {
        assert_eq!(run(text), Ok(expected), "{text}");
    }
    for text in [
        "SQRT(-1)",
        "LN(0)",
        "LPAD('a', 200000)",
        "DATETIMEVALUE('x')",
    ] {
        assert!(run(text).is_err(), "{text}");
    }
    let fixture = fixture();
    for (text, fragment) in [
        ("REGEX(name, '(')", "not a valid regular expression"),
        ("REGEX(name, custom:stage)", "in quotes"),
        ("REGEX(custom:amount, 'a')", "needs text"),
        ("FIND('a')", "FIND takes 2 to 3 arguments"),
    ] {
        let error = compile_as(&fixture, text, Purpose::Save).expect_err(text);
        assert!(error.message.contains(fragment), "{text}: {error:?}");
    }
}
