// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use serde_json::json;

#[test]
fn cancel_is_final_and_every_failure_falls_through() {
    assert!(final_outcome(&Some(Outcome::Cancelled)));
    assert!(!final_outcome(&Some(Outcome::Failed("failure".into()))));
    assert!(!final_outcome(&None));
}

#[test]
fn chosen_preserves_paths_filter_and_choice_defaults() {
    let request = request(&json!({"multiple":true,"filters":[{"name":"Text","patterns":["*.md"]}],
        "choices":[{"id":"encoding","label":"Encoding","options":[{"id":"utf8","label":"UTF-8"}]},
        {"id":"readonly","label":"Read only","options":[]}]}).to_string()).unwrap();
    let outcome = decode_outcome(&request, &json!({"response":0,"uris":["file:///tmp/%FF.md","file:///tmp/two.md"]}));
    let Outcome::Chosen {paths, choices, current_filter} = outcome else { panic!("not chosen") };
    assert_eq!(path_bytes(&paths[0]), b"/tmp/\xff.md");
    assert_eq!(choices["encoding"], "utf8");
    assert_eq!(choices["readonly"], "false");
    assert_eq!(current_filter, request.current_filter);
    assert!(final_outcome(&Some(Outcome::Chosen {paths, choices, current_filter})));
}

#[test]
fn bus_outcome_refuses_shapes_not_offered() {
    let request = request("{}").unwrap();
    assert!(matches!(decode_outcome(&request, &json!({"response":0,"uris":[]})), Outcome::Failed(_)));
    assert!(matches!(decode_outcome(&request, &json!({"response":1})), Outcome::Cancelled));
    assert!(matches!(decode_outcome(&request, &json!({"response":2})), Outcome::Failed(_)));
}
