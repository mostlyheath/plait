use plait_protocol::host::{
    build_cache::{ClearReply, Status as BuildCacheStatus},
    folders::Listing,
    release::{CancelReply, DrainReply, HostInfo},
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

fn round_trip<T: DeserializeOwned + Serialize>(value: Value) {
    let reply: T = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(reply).unwrap(), value);
}

#[test]
fn recorded_reply_shapes_round_trip() {
    round_trip::<Listing>(
        json!({"path":"/srv","parent":"/","home":"/home/me","git":true,"entries":[{"name":"repo","git":false}],"truncated":false}),
    );
    round_trip::<HostInfo>(
        json!({"schema":1,"release":null,"build_id":null,"protocol_min":1,"protocol_max":6,"capabilities":["drain_v1"],"storage_schema":1}),
    );
    round_trip::<DrainReply>(json!({"draining":true,"active_runs":1}));
    round_trip::<CancelReply>(json!({"draining":false}));
    round_trip::<BuildCacheStatus>(
        json!({"schema":1,"total_bytes":4200000000u64,"idle_bytes":3000000000u64,"thread_count":2,"active_count":1,"cap_bytes":20000000000u64,"measuring":false}),
    );
    round_trip::<ClearReply>(
        json!({"schema":1,"cleared_threads":1,"cleared_bytes":3000000000u64,"skipped_active":1,"total_bytes":1200000000u64,"idle_bytes":0,"thread_count":1,"active_count":1,"cap_bytes":20000000000u64,"measuring":false}),
    );
}

#[test]
fn build_cache_replies_ignore_unknown_fields_and_default_measuring() {
    let status: BuildCacheStatus = serde_json::from_value(json!({
        "schema": 1,
        "total_bytes": 100,
        "idle_bytes": 50,
        "thread_count": 2,
        "active_count": 1,
        "cap_bytes": 1000,
        "future_field": "ignored"
    }))
    .unwrap();
    assert!(!status.measuring);

    let clear: ClearReply = serde_json::from_value(json!({
        "schema": 1,
        "cleared_threads": 1,
        "cleared_bytes": 50,
        "skipped_active": 1,
        "total_bytes": 50,
        "idle_bytes": 0,
        "thread_count": 1,
        "active_count": 1,
        "cap_bytes": 1000,
        "future_field": true
    }))
    .unwrap();
    assert!(!clear.measuring);
    assert!(!clear.status().measuring);
}

#[test]
fn optional_turn_outcome_accepts_future_values() {
    use plait_protocol::host::{TurnOutcome, TurnRecord};
    let mut value = json!({"id":"r","request_ids":["q"],"request_ids_offset":0,"request_ids_total":1,"kind":"turn","state":"ready","head_moved":false,"after_restart":false,"paths":[],"paths_offset":0,"paths_total":0,"not_covered":[],"not_covered_offset":0,"not_covered_total":0,"created_at":1});
    let decode = |v: &Value| {
        serde_json::from_value::<TurnRecord>(v.clone())
            .unwrap()
            .outcome
    };
    assert_eq!(decode(&value), None);
    for (wire, expected) in [
        ("finished", Some(TurnOutcome::Finished)),
        ("failed", Some(TurnOutcome::Failed)),
        ("stopped", Some(TurnOutcome::Stopped)),
        ("interrupted", Some(TurnOutcome::Interrupted)),
        ("future", None),
    ] {
        value["outcome"] = json!(wire);
        assert_eq!(decode(&value), expected);
    }
}
