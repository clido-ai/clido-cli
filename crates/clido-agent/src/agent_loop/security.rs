//! Prompt injection detection and tool error formatting.

/// Detect potential prompt injection patterns in tool arguments.
/// Returns the matched pattern category if suspicious content is found.
pub(crate) fn detect_injection(input: &serde_json::Value) -> Option<&'static str> {
    let text = match input {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(_) => serde_json::to_string(input).unwrap_or_default(),
        _ => return None,
    };
    let lower = text.to_lowercase();

    static PATTERNS: &[(&str, &str)] = &[
        ("ignore previous instructions", "instruction override"),
        ("ignore all previous", "instruction override"),
        ("disregard previous", "instruction override"),
        ("forget your instructions", "instruction override"),
        ("you are now", "role hijacking"),
        ("new system prompt", "system prompt injection"),
        ("override system prompt", "system prompt injection"),
        ("<system>", "XML tag injection"),
        ("</system>", "XML tag injection"),
        ("<|im_start|>", "chat template injection"),
        ("<|im_end|>", "chat template injection"),
        ("human:", "role boundary injection"),
        ("assistant:", "role boundary injection"),
        ("[inst]", "instruction tag injection"),
        ("[/inst]", "instruction tag injection"),
    ];

    for (pattern, category) in PATTERNS {
        if lower.contains(pattern) {
            return Some(category);
        }
    }
    None
}

/// Wrap a tool error in structured feedback to help the model self-correct.
pub(crate) fn format_tool_error_for_reflection(tool_name: &str, error_output: &str) -> String {
    format!(
        "[Tool Error] The {} tool returned an error:\n{}\n\nPlease analyze what went wrong and try a corrected approach.",
        tool_name, error_output
    )
}

/// Build an enhanced error message for edit tool failures that includes
/// the target file's content around the failed region to help the model self-correct.
pub(crate) fn enhanced_edit_error(
    tool_name: &str,
    error_output: &str,
    input: &serde_json::Value,
) -> String {
    if !matches!(tool_name, "Edit" | "MultiEdit") {
        return format_tool_error_for_reflection(tool_name, error_output);
    }

    let file_path = input
        .get("file_path")
        .or_else(|| input.get("path"))
        .and_then(|v| v.as_str());

    let file_context = if let Some(path) = file_path {
        match std::fs::read_to_string(path) {
            Ok(content) => {
                let lines: Vec<&str> = content.lines().collect();
                let total = lines.len();
                if total <= 100 {
                    format!(
                        "\n\n[File Content ({} lines)]:\n```\n{}\n```",
                        total, content
                    )
                } else {
                    let head: String = lines[..30].join("\n");
                    let tail: String = lines[total - 30..].join("\n");
                    format!(
                        "\n\n[File Excerpt ({} lines total)]:\n```\n{}\n... ({} lines omitted) ...\n{}\n```",
                        total, head, total - 60, tail
                    )
                }
            }
            Err(_) => String::new(),
        }
    } else {
        String::new()
    };

    let old_str_hint = input
        .get("old_string")
        .or_else(|| input.get("old_str"))
        .and_then(|v| v.as_str())
        .map(|s| format!("\n\n[You searched for]:\n```\n{}\n```", s))
        .unwrap_or_default();

    format!(
        "[Tool Error] The {} tool returned an error:\n{}{}{}\n\n\
         Hint: Re-read the file with the Read tool to see the current content, \
         then retry with the exact text from the file. Do NOT guess the content.",
        tool_name, error_output, old_str_hint, file_context
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn detect_injection_finds_known_patterns() {
        assert_eq!(
            detect_injection(&json!("Please ignore previous instructions")),
            Some("instruction override")
        );
        assert_eq!(
            detect_injection(&json!({"x": "new system prompt here"})),
            Some("system prompt injection")
        );
        assert_eq!(detect_injection(&json!(42)), None);
    }

    #[test]
    fn format_tool_error_includes_name_and_hint() {
        let s = format_tool_error_for_reflection("Read", "file missing");
        assert!(s.contains("Read") && s.contains("file missing"));
    }

    #[test]
    fn enhanced_edit_non_edit_delegates() {
        let s = enhanced_edit_error("Read", "oops", &json!({}));
        assert!(s.contains("Read") && s.contains("oops"));
    }

    #[test]
    fn enhanced_edit_short_file_includes_full_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        std::fs::write(&path, "line1\nline2\n").unwrap();
        let input = json!({"file_path": path.to_str().unwrap()});
        let s = enhanced_edit_error("Edit", "no match", &input);
        assert!(s.contains("line1"));
    }

    #[test]
    fn enhanced_edit_long_file_shows_excerpt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.txt");
        let lines: String = (0..120).map(|i| format!("L{i}\n")).collect();
        std::fs::write(&path, &lines).unwrap();
        let input = json!({"path": path.to_str().unwrap(), "old_string": "needle"});
        let s = enhanced_edit_error("MultiEdit", "fail", &input);
        assert!(s.contains("omitted"));
        assert!(s.contains("L0"));
        assert!(s.contains("L119"));
    }
}
