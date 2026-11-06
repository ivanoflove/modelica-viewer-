use std::collections::HashMap;

/// Static context used when resolving Modelica `Text(textString=...)`
/// templates. The maps contain only values that can be obtained without
/// evaluating arbitrary Modelica expressions.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ModelTextContext {
    pub class_qualified_name: String,
    pub class_name: String,
    pub instance_name: String,
    pub parameter_defaults: HashMap<String, String>,
    pub parameter_bindings: HashMap<String, String>,
}

impl ModelTextContext {
    pub fn new(
        class_qualified_name: impl Into<String>,
        class_name: impl Into<String>,
        instance_name: impl Into<String>,
        parameter_defaults: HashMap<String, String>,
        parameter_bindings: HashMap<String, String>,
    ) -> Self {
        Self {
            class_qualified_name: class_qualified_name.into(),
            class_name: class_name.into(),
            instance_name: instance_name.into(),
            parameter_defaults,
            parameter_bindings,
        }
    }

    pub fn value_for(&self, name: &str) -> Option<&str> {
        self.parameter_bindings
            .get(name)
            .or_else(|| self.parameter_defaults.get(name))
            .map(String::as_str)
    }
}

/// Resolve the Modelica text macros supported by the viewer.
///
/// Unknown macros are intentionally preserved so a missing static parameter
/// never turns a visible label into an empty string.
pub fn resolve_modelica_text(template: &str, context: &ModelTextContext) -> String {
    let mut output = String::with_capacity(template.len());
    let chars = template.chars().collect::<Vec<_>>();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] != '%' {
            output.push(chars[index]);
            index += 1;
            continue;
        }
        if chars.get(index + 1) == Some(&'%') {
            output.push('%');
            index += 2;
            continue;
        }

        let braced = chars.get(index + 1) == Some(&'{');
        let (start, mut end) = if braced {
            let start = index + 2;
            let end = chars[start..]
                .iter()
                .position(|character| *character == '}')
                .map_or(start, |offset| start + offset);
            (start, end)
        } else {
            let start = index + 1;
            let mut end = start;
            while chars
                .get(end)
                .is_some_and(|character| character.is_ascii_alphanumeric() || *character == '_')
            {
                end += 1;
            }
            (start, end)
        };

        if end == start {
            output.push('%');
            index += 1;
            continue;
        }
        let key = chars[start..end].iter().collect::<String>();
        if braced && chars.get(end) == Some(&'}') {
            end += 1;
        }
        match key.as_str() {
            "name" => output.push_str(&context.instance_name),
            "class" => output.push_str(&context.class_name),
            _ => match context.value_for(&key) {
                Some(value) => output.push_str(value),
                None => append_unresolved_macro(&mut output, &key, braced),
            },
        }
        index = end;
    }
    output
}

fn append_unresolved_macro(output: &mut String, key: &str, braced: bool) {
    output.push('%');
    if braced {
        output.push('{');
    }
    output.push_str(key);
    if braced {
        output.push('}');
    }
}

#[cfg(test)]
mod tests {
    use super::{ModelTextContext, resolve_modelica_text};
    use std::collections::HashMap;

    fn context() -> ModelTextContext {
        ModelTextContext::new(
            "Demo.Component",
            "Component",
            "instance",
            HashMap::from([(String::from("gain"), String::from("2"))]),
            HashMap::new(),
        )
    }

    #[test]
    fn resolves_standard_macros_and_static_parameter_defaults() {
        assert_eq!(
            resolve_modelica_text("%% %name %class %gain %{gain}", &context()),
            "% instance Component 2 2"
        );
    }

    #[test]
    fn parameter_binding_overrides_default() {
        let mut context = context();
        context
            .parameter_bindings
            .insert("gain".to_owned(), "5".to_owned());
        assert_eq!(resolve_modelica_text("%gain", &context), "5");
    }

    #[test]
    fn unknown_macros_are_preserved() {
        assert_eq!(
            resolve_modelica_text("%unknown %{missing}", &context()),
            "%unknown %{missing}"
        );
    }
}
