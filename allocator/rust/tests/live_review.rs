use lab_allocator::lease_review;
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

#[test]
#[ignore = "requires a SQLite-consistent staging snapshot and its review responses from the deployed allocator"]
fn review_matches_actual_staging_lease_data_across_implementations() {
    let database =
        std::env::var_os("LAB_ALLOCATOR_REVIEW_SNAPSHOT").expect("staging snapshot path");
    let expected =
        std::env::var_os("LAB_ALLOCATOR_LEGACY_REVIEW").expect("staging review evidence path");
    let expected: Value = serde_json::from_slice(&std::fs::read(expected).unwrap()).unwrap();
    let now = expected["as_of"].as_f64().unwrap();
    let mut db = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    for scope in ["counts", "current", "issues", "released", "all"] {
        let actual = lease_review(&mut db, None, scope, now).unwrap();
        // SQLite REAL fields round-trip from a Python JSON response. Compare
        // values numerically across serializers while other values stay exact.
        compare(&actual, &expected[scope], scope);
    }
    assert!(
        expected["counts"]["counts"]["released"].as_i64().unwrap() > 0,
        "real retained lease history was exercised"
    );
}

fn compare(value: &Value, expected: &Value, path: &str) {
    match (value, expected) {
        (Value::Number(actual), Value::Number(expected)) => {
            let actual = actual.as_f64().unwrap();
            let expected = expected.as_f64().unwrap();
            assert!(
                (actual - expected).abs() < 1e-6,
                "number diverged at {path}"
            );
        }
        (Value::Array(actual), Value::Array(expected)) => {
            assert_eq!(actual.len(), expected.len(), "length differed at {path}");
            for (index, (value, expected)) in actual.iter().zip(expected.iter()).enumerate() {
                compare(value, expected, &format!("{path}[{index}]"));
            }
        }
        (Value::Object(actual), Value::Object(expected)) => {
            assert!(
                actual.keys().eq(expected.keys()),
                "fields diverged at {path}"
            );
            for (field, value) in actual {
                compare(value, &expected[field], &format!("{path}.{field}"));
            }
        }
        _ => assert!(value == expected, "value diverged at {path}"),
    }
}
