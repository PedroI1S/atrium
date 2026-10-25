//! Model selection state as an ACP agent reports it.
//!
//! ACP has two ways for an agent to expose its model: a `configOptions` entry
//! with `category: "model"` (stable; set with `session/set_config_option`) or
//! the older `models` state (`session/set_model`). OpenCode v2.1.3 uses the
//! first one. Both the adapter (to decide whether it has to select a model)
//! and the normalizer (to decide when the session counts as started) use the
//! same predicate, so a live run and a replay agree.

use serde_json::Value;

/// How the agent lets a client change the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelVia {
    /// `session/set_config_option` with this `configId`.
    ConfigOption { config_id: String },
    /// `session/set_model` (the `models` state).
    ModelsState,
}

/// The model state found in a `session/new` / `load` / `resume` /
/// `set_config_option` result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelState {
    pub via: ModelVia,
    pub current: String,
    /// Every value the agent offers. Empty when the agent did not list them.
    pub available: Vec<String>,
}

impl ModelState {
    /// True if the agent is running `requested`, and (when it lists its
    /// models) `requested` is one of them. A current value that is not an
    /// offered model is treated as "not selected": an agent that echoes back
    /// an unknown id has not actually validated it.
    pub fn confirms(&self, requested: &str) -> bool {
        self.current == requested
            && (self.available.is_empty() || self.available.iter().any(|m| m == requested))
    }
}

/// Reads the model state out of a result object, if the agent reports one.
pub fn parse_model_state(result: &Value) -> Option<ModelState> {
    if let Some(options) = result.get("configOptions").and_then(Value::as_array) {
        for opt in options {
            let id = opt.get("id").and_then(Value::as_str).unwrap_or_default();
            let category = opt.get("category").and_then(Value::as_str);
            if id != "model" && category != Some("model") {
                continue;
            }
            let Some(current) = opt.get("currentValue").and_then(Value::as_str) else {
                continue;
            };
            let mut available = Vec::new();
            collect_select_values(opt.get("options"), &mut available);
            return Some(ModelState {
                via: ModelVia::ConfigOption {
                    config_id: id.to_owned(),
                },
                current: current.to_owned(),
                available,
            });
        }
    }
    let models = result.get("models")?;
    let current = models.get("currentModelId").and_then(Value::as_str)?;
    let available = models
        .get("availableModels")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("modelId").and_then(Value::as_str))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    Some(ModelState {
        via: ModelVia::ModelsState,
        current: current.to_owned(),
        available,
    })
}

/// A select option's `options` is either a flat list of `{value, name}` or a
/// list of groups `{group, name, options: [...]}`.
fn collect_select_values(options: Option<&Value>, out: &mut Vec<String>) {
    let Some(arr) = options.and_then(Value::as_array) else {
        return;
    };
    for item in arr {
        if let Some(v) = item.get("value").and_then(Value::as_str) {
            out.push(v.to_owned());
        } else {
            collect_select_values(item.get("options"), out);
        }
    }
}

/// Finds a config option by category (`"mode"`, `"thought_level"`, ...) or id
/// and returns `(config_id, current, offered values)`.
pub fn find_select_option(
    result: &Value,
    category: &str,
    id: &str,
) -> Option<(String, String, Vec<String>)> {
    let options = result.get("configOptions")?.as_array()?;
    for opt in options {
        let opt_id = opt.get("id").and_then(Value::as_str).unwrap_or_default();
        if opt_id != id && opt.get("category").and_then(Value::as_str) != Some(category) {
            continue;
        }
        let current = opt.get("currentValue").and_then(Value::as_str)?;
        let mut available = Vec::new();
        collect_select_values(opt.get("options"), &mut available);
        return Some((opt_id.to_owned(), current.to_owned(), available));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_opencode_style_config_option() {
        let result = json!({"sessionId": "s", "configOptions": [
            {"id": "model", "category": "model", "type": "select", "currentValue": "a/b",
             "options": [{"value": "a/b", "name": "B"}, {"value": "c/d", "name": "D"}]},
            {"id": "effort", "category": "thought_level", "type": "select",
             "currentValue": "default", "options": [{"value": "low", "name": "Low"}]}
        ]});
        let state = parse_model_state(&result).expect("model state");
        assert_eq!(
            state.via,
            ModelVia::ConfigOption {
                config_id: "model".into()
            }
        );
        assert!(state.confirms("a/b"));
        assert!(!state.confirms("c/d"), "offered but not current");
        assert!(!state.confirms("x/y"));
        let (id, cur, avail) = find_select_option(&result, "thought_level", "effort").unwrap();
        assert_eq!((id.as_str(), cur.as_str()), ("effort", "default"));
        assert_eq!(avail, vec!["low".to_owned()]);
    }

    #[test]
    fn current_value_that_is_not_offered_is_not_confirmed() {
        let result = json!({"configOptions": [
            {"id": "model", "category": "model", "type": "select",
             "currentValue": "bad/model", "options": [{"value": "a/b", "name": "B"}]}
        ]});
        assert!(!parse_model_state(&result).unwrap().confirms("bad/model"));
    }

    #[test]
    fn grouped_options_and_models_state() {
        let grouped = json!({"configOptions": [
            {"id": "model", "category": "model", "type": "select", "currentValue": "a/b",
             "options": [{"group": "g", "name": "G", "options": [{"value": "a/b", "name": "B"}]}]}
        ]});
        assert!(parse_model_state(&grouped).unwrap().confirms("a/b"));
        let models = json!({"models": {"currentModelId": "m1",
            "availableModels": [{"modelId": "m1", "name": "M1"}, {"modelId": "m2", "name": "M2"}]}});
        let state = parse_model_state(&models).unwrap();
        assert_eq!(state.via, ModelVia::ModelsState);
        assert!(state.confirms("m1") && !state.confirms("m2"));
    }

    #[test]
    fn no_model_state_when_agent_reports_none() {
        assert!(parse_model_state(&json!({"sessionId": "s"})).is_none());
    }
}
