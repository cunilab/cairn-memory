use super::model::Kind;
use crate::error::Error;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Every per-kind struct omits empty values when serialized, so the stored object stays minimal.
/// `default` beside `skip_serializing_if` is what makes ts-rs type the key as optional (`key?:`).
macro_rules! fields_struct {
    ($name:ident { $($(#[$attr:meta])* $field:ident : $ty:ty),* $(,)? }) => {
        #[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
        #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(default, deny_unknown_fields)]
        pub struct $name {
            $(#[serde(default, skip_serializing_if = "is_empty")] $(#[$attr])* pub $field: $ty,)*
        }
    };
}

trait Blank {
    fn blank(&self) -> bool;
}
impl Blank for String {
    fn blank(&self) -> bool {
        self.is_empty()
    }
}
impl Blank for Vec<String> {
    fn blank(&self) -> bool {
        self.is_empty()
    }
}
fn is_empty<T: Blank>(value: &T) -> bool {
    value.blank()
}

fn label(
    out: &mut Vec<(&'static str, &'static str, String)>,
    key: &'static str,
    name: &'static str,
    text: &str,
) {
    if !text.is_empty() {
        out.push((key, name, text.to_owned()));
    }
}

fields_struct!(Decision { context: String, alternatives: Vec<String>, consequences: String });
fields_struct!(Rule { exceptions: String });
fields_struct!(Gotcha {
    symptom: String,
    cause: String,
    fix: String
});
// `steps` is ordered.
fields_struct!(Procedure { steps: Vec<String>, when: String });
fields_struct!(Fact { source: String });
fields_struct!(Preference { example: String });
fields_struct!(Note {});
fields_struct!(Handoff { next_steps: Vec<String>, open_questions: Vec<String>, files: Vec<String> });

/// Serialize only; the kind travels in `Memory::kind`.
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Fields {
    Decision(Decision),
    Rule(Rule),
    Gotcha(Gotcha),
    Procedure(Procedure),
    Fact(Fact),
    Preference(Preference),
    Note(Note),
    Handoff(Handoff),
}

impl Fields {
    /// Wrong or unknown keys for the kind are a validation error (400).
    pub fn parse(kind: Kind, value: Option<Value>) -> Result<Self, Error> {
        let value = match value {
            None | Some(Value::Null) => Value::Object(Default::default()),
            Some(value @ Value::Object(_)) => value,
            Some(_) => return Err(Error::validation("fields must be an object")),
        };
        let wrong = |error: serde_json::Error| {
            Error::validation(format!("fields for {}: {error}", kind.as_str()))
        };
        Ok(match kind {
            Kind::Decision => Self::Decision(serde_json::from_value(value).map_err(wrong)?),
            Kind::Rule => Self::Rule(serde_json::from_value(value).map_err(wrong)?),
            Kind::Gotcha => Self::Gotcha(serde_json::from_value(value).map_err(wrong)?),
            Kind::Procedure => Self::Procedure(serde_json::from_value(value).map_err(wrong)?),
            Kind::Fact => Self::Fact(serde_json::from_value(value).map_err(wrong)?),
            Kind::Preference => Self::Preference(serde_json::from_value(value).map_err(wrong)?),
            Kind::Note => Self::Note(serde_json::from_value(value).map_err(wrong)?),
            Kind::Handoff => Self::Handoff(serde_json::from_value(value).map_err(wrong)?),
        })
    }

    /// True when `to_json()` is `{}`.
    pub fn is_empty(&self) -> bool {
        self.to_json().as_object().is_none_or(|o| o.is_empty())
    }

    pub fn kind(&self) -> Kind {
        match self {
            Self::Decision(_) => Kind::Decision,
            Self::Rule(_) => Kind::Rule,
            Self::Gotcha(_) => Kind::Gotcha,
            Self::Procedure(_) => Kind::Procedure,
            Self::Fact(_) => Kind::Fact,
            Self::Preference(_) => Kind::Preference,
            Self::Note(_) => Kind::Note,
            Self::Handoff(_) => Kind::Handoff,
        }
    }

    /// The bare per-kind object; empty strings and arrays are omitted.
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| Value::Object(Default::default()))
    }

    /// Each stored string on its own, with a stable key and a short label. Array items stay separate.
    pub fn labeled(&self) -> Vec<(&'static str, &'static str, String)> {
        let mut out = Vec::new();
        match self {
            Self::Decision(value) => {
                label(&mut out, "context", "Context", &value.context);
                for item in &value.alternatives {
                    label(&mut out, "alternatives", "Alternatives", item);
                }
                label(
                    &mut out,
                    "consequences",
                    "Consequences",
                    &value.consequences,
                );
            }
            Self::Rule(value) => label(&mut out, "exceptions", "Exceptions", &value.exceptions),
            Self::Gotcha(value) => {
                label(&mut out, "symptom", "Symptom", &value.symptom);
                label(&mut out, "cause", "Cause", &value.cause);
                label(&mut out, "fix", "Fix", &value.fix);
            }
            Self::Procedure(value) => {
                for item in &value.steps {
                    label(&mut out, "steps", "Steps", item);
                }
                label(&mut out, "when", "When", &value.when);
            }
            Self::Fact(value) => label(&mut out, "source", "Source", &value.source),
            Self::Preference(value) => label(&mut out, "example", "Example", &value.example),
            Self::Note(_) => {}
            Self::Handoff(value) => {
                for item in &value.next_steps {
                    label(&mut out, "next_steps", "Next steps", item);
                }
                for item in &value.open_questions {
                    label(&mut out, "open_questions", "Open questions", item);
                }
                for item in &value.files {
                    label(&mut out, "files", "Files", item);
                }
            }
        }
        out
    }

    /// All text values in key order, one per line. Used for embedding text and search.
    pub fn text(&self) -> String {
        fn collect(value: &Value, out: &mut Vec<String>) {
            match value {
                Value::String(text) => out.push(text.clone()),
                Value::Array(items) => items.iter().for_each(|item| collect(item, out)),
                Value::Object(map) => map.values().for_each(|item| collect(item, out)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        collect(&self.to_json(), &mut out);
        out.join("\n")
    }

    /// Applies `f` to every string, for redaction.
    pub fn map_text(self, f: impl Fn(&str) -> String) -> Self {
        let kind = self.kind();
        let mut value = self.to_json();
        fn walk(value: &mut Value, f: &dyn Fn(&str) -> String) {
            match value {
                Value::String(text) => *text = f(text),
                Value::Array(items) => items.iter_mut().for_each(|item| walk(item, f)),
                Value::Object(map) => map.values_mut().for_each(|item| walk(item, f)),
                _ => {}
            }
        }
        walk(&mut value, &f);
        // The shape came from this kind's own serializer, so it parses back.
        Self::parse(kind, Some(value)).unwrap_or(self)
    }
}
