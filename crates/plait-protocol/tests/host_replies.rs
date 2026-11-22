use plait_protocol::host::{
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
}
