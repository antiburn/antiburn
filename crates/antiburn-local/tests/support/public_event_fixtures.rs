use serde_json::{Value, json};

pub fn codex_incomplete_output(output: Value) -> String {
    include_str!("../fixtures/public_events/codex.jsonl")
        .lines()
        .map(|line| {
            let mut record: Value = serde_json::from_str(line).unwrap();
            if record["payload"]["type"] == "custom_tool_call_output" {
                record["payload"]["output"] = output.clone();
            }
            format!("{record}\n")
        })
        .collect()
}

pub fn codex_replaced_call(name: &str) -> String {
    include_str!("../fixtures/codex_characterization/paginated_completed.jsonl")
        .replace("synthetic-root", "facts")
        .lines()
        .map(|line| {
            let mut record: Value = serde_json::from_str(line).unwrap();
            if record["payload"]["type"] == "custom_tool_call" {
                record["payload"]["name"] = json!(name);
                if name != "exec" {
                    record["payload"]["type"] = json!("function_call");
                    record["payload"].as_object_mut().unwrap().remove("input");
                    record["payload"]["arguments"] = json!(if name == "exec_command" {
                        json!({"cmd":"synthetic command"}).to_string()
                    } else {
                        json!({"patch":"*** Begin Patch\n*** End Patch"}).to_string()
                    });
                }
            }
            if name != "exec" && record["payload"]["type"] == "custom_tool_call_output" {
                record["payload"]["type"] = json!("function_call_output");
            }
            format!("{record}\n")
        })
        .collect()
}

pub fn pi_completion_marker(owner: &str, field: &str, marker: Value) -> String {
    include_str!("../fixtures/public_events/pi.jsonl")
        .lines()
        .map(|line| {
            let mut record: Value = serde_json::from_str(line).unwrap();
            if record["id"] == "assistant-1" {
                let value = match owner {
                    "record" => &mut record,
                    "message" => &mut record["message"],
                    "call" => &mut record["message"]["content"][2],
                    "text" => &mut record["message"]["content"][1],
                    "private" => &mut record["message"]["content"][0],
                    _ => panic!("unknown completion marker owner"),
                };
                value[field] = marker.clone();
            }
            format!("{record}\n")
        })
        .collect()
}
