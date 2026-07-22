use native_config_fixtures::normalize_app_config;
use std::fs;
use std::path::PathBuf;

fn fixture(name: &str) -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    fs::read_to_string(root.join("docs/config-fixtures").join(name)).expect("fixture")
}

fn with_default_fields(mut value: serde_json::Value) -> serde_json::Value {
    if let Some(users) = value["users"].as_object_mut() {
        for profile in users.values_mut() {
            if let Some(profile) = profile.as_object_mut() {
                profile
                    .entry("excluded_real_paths")
                    .or_insert_with(|| serde_json::json!([]));
            }
        }
    }
    value
}

#[test]
fn normalized_app_fixture_matches_native_runtime_contract() {
    let input = fixture("app-profile-normalization-output.json");
    let expected: serde_json::Value = serde_json::from_str(&input).expect("expected fixture");
    assert_eq!(
        normalize_app_config("com.example", &input),
        with_default_fields(expected)
    );
}

#[test]
fn full_app_fixture_matches_native_runtime_contract() {
    let input = fixture("app-profile-full.json");
    let expected: serde_json::Value = serde_json::from_str(&input).expect("expected fixture");
    assert_eq!(
        normalize_app_config("com.example", &input),
        with_default_fields(expected)
    );
}

#[test]
fn native_runtime_rejects_cyclic_mapping_chains() {
    let input = r#"{"users":{"0":{"path_mappings":{"A":"B","B":"C","C":"A","Keep":"Target"}}}}"#;
    let normalized = normalize_app_config("com.example", input);
    assert_eq!(
        normalized["users"]["0"]["path_mappings"],
        serde_json::json!({ "Keep": "Target" })
    );
}
