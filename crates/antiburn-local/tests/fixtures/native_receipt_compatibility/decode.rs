pub fn expand_sparse_arrays(value: &mut serde_json::Value) {
    use serde_json::{Value, json};
    if let Some(sparse) = value.get("fixture_sparse_u64_array") {
        let len = usize::try_from(sparse["len"].as_u64().unwrap()).unwrap();
        assert!(len <= 131_072);
        let mut items = vec![json!(0); len];
        for entry in sparse["nonzero"].as_array().unwrap() {
            let index = usize::try_from(entry[0].as_u64().unwrap()).unwrap();
            assert!(entry[1].as_u64().is_some());
            items[index] = entry[1].clone();
        }
        *value = Value::Array(items);
    } else {
        match value {
            Value::Array(items) => items.iter_mut().for_each(expand_sparse_arrays),
            Value::Object(fields) => fields.values_mut().for_each(expand_sparse_arrays),
            _ => {}
        }
    }
}
