use std::path::Path;
use zatara_types::LaunchConfig;
pub(crate) struct TestCase {
    pub display: String,
    pub id: String,
}
pub(crate) fn list_tail_args() -> [&'static str; 3] {
    ["-preEnumerateTheories", "-list", "full/json"]
}
pub(crate) fn id_args(id: &str) -> Vec<String> {
    ["-preEnumerateTheories", "-id", id]
        .into_iter()
        .map(str::to_string)
        .collect()
}
pub(crate) fn normalize_arg(raw: &str) -> String {
    let trimmed = raw.trim();
    match trimmed {
        "true" => "True".to_string(),
        "false" => "False".to_string(),
        other => last_segment_if_qualified_identifier(other).to_string(),
    }
}
fn last_segment_if_qualified_identifier(value: &str) -> &str {
    let is_segment = |s: &str| {
        let mut chars = s.chars();
        matches!(chars.next(), Some(c) if c.is_alphabetic() || c == '_')
            && chars.all(|c| c.is_alphanumeric() || c == '_')
    };
    if value.contains('.') && value.split('.').all(is_segment) {
        value.rsplit('.').next().expect("a dotted all-identifier has a segment")
    } else {
        value
    }
}
pub(crate) fn case_display_name(fqn: &str, params: &[String], raws: &[String]) -> String {
    let body = raws
        .iter()
        .enumerate()
        .map(|(i, raw)| {
            let value = normalize_arg(raw);
            match params.get(i) {
                Some(param) if !param.is_empty() => format!("{param}: {value}"),
                _ => value,
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("{fqn}({body})")
}
pub(crate) fn parse_cases(stdout: &str) -> Vec<TestCase> {
    let (Some(start), Some(end)) = (stdout.find('['), stdout.rfind(']')) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&stdout[start..=end]) else {
        return Vec::new();
    };
    let Some(items) = value.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let display = item.get("DisplayName")?.as_str()?.to_string();
            let id = item.get("ID")?.as_str()?.to_string();
            Some(TestCase { display, id })
        })
        .collect()
}
pub(crate) fn find_case_id(cases: &[TestCase], config: &LaunchConfig) -> Result<String, String> {
    let target = case_display_name(
        &config.fully_qualified_name,
        &config.method_parameters,
        &config.case_raw_arguments,
    );
    let hits: Vec<&TestCase> = cases.iter().filter(|c| c.display == target).collect();
    match hits.len() {
        1 => Ok(hits[0].id.clone()),
        0 => Err(format!(
            "no xunit v3 test case matches `{target}` — the row's arguments matched no \
             enumerated case (custom test display names, or a complex argument the \
             gutter cannot name — flag/cast enum expressions, member-data shapes); \
             click the method's gutter arrow to run every case of `{}`",
            config.fully_qualified_name
        )),
        n => Err(format!(
            "{n} xunit v3 test cases match `{target}` — the row is ambiguous; click the \
             method's gutter arrow to run every case of `{}`",
            config.fully_qualified_name
        )),
    }
}
pub(crate) fn enumerate(dll: &Path) -> Result<Vec<TestCase>, String> {
    let out = std::process::Command::new("dotnet")
        .arg(dll)
        .args(list_tail_args())
        .output()
        .map_err(|e| format!("failed to spawn `dotnet {} -list`: {e}", dll.display()))?;
    Ok(parse_cases(&String::from_utf8_lossy(&out.stdout)))
}
pub(crate) fn resolve_case_id(dll: &Path, config: &LaunchConfig) -> Result<String, String> {
    find_case_id(&enumerate(dll)?, config)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn config(params: &[&str], raws: &[&str]) -> LaunchConfig {
        LaunchConfig {
            fully_qualified_name: "Probe.Rows.Mixed3".to_string(),
            method_parameters: params.iter().map(|s| s.to_string()).collect(),
            case_raw_arguments: raws.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }
    fn cases(rows: &[(&str, &str)]) -> Vec<TestCase> {
        rows.iter()
            .map(|(display, id)| TestCase {
                display: display.to_string(),
                id: id.to_string(),
            })
            .collect()
    }
    #[test]
    fn the_two_retitles_are_bool_casing_and_the_qualifed_enum_short_name() {
        assert_eq!(normalize_arg("true"), "True");
        assert_eq!(normalize_arg("false"), "False");
        assert_eq!(normalize_arg("Color.Green"), "Green");
        assert_eq!(normalize_arg("Ns.Deep.Color.Red"), "Red");
        assert_eq!(normalize_arg("\"solo\""), "\"solo\"");
        assert_eq!(normalize_arg("'x'"), "'x'");
        assert_eq!(normalize_arg("null"), "null");
        assert_eq!(normalize_arg("1.5"), "1.5");
        assert_eq!(normalize_arg("(Color)1"), "(Color)1");
        assert_eq!(normalize_arg("Color.Red | Color.Green"), "Color.Red | Color.Green");
        assert_eq!(normalize_arg("1.2.3"), "1.2.3");
    }
    #[test]
    fn the_display_name_is_the_method_fqn_plus_param_value_pairs() {
        let name = case_display_name(
            "Probe.Rows.Mixed3",
            &["s".into(), "n".into(), "flag".into()],
            &["\"a\"".into(), "1".into(), "true".into()],
        );
        assert_eq!(name, "Probe.Rows.Mixed3(s: \"a\", n: 1, flag: True)");
    }
    #[test]
    fn the_list_json_is_parsed_to_display_id_pairs() {
        let stdout = "xUnit.net v3 In-Process Runner\n\
             [{\"DisplayName\":\"Probe.Rows.Ints(n: 1)\",\"ID\":\"aaa\"},\n\
              {\"DisplayName\":\"Probe.Rows.Ints(n: 42)\",\"ID\":\"bbb\"}]\n";
        let parsed = parse_cases(stdout);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].display, "Probe.Rows.Ints(n: 1)");
        assert_eq!(parsed[1].id, "bbb");
    }
    #[test]
    fn stdout_without_a_json_array_parses_empty() {
        assert!(parse_cases("error: unknown option").is_empty());
    }
    #[test]
    fn a_matching_row_resolves_to_its_id() {
        let found = find_case_id(
            &cases(&[
                ("Probe.Rows.Mixed3(s: \"a\", n: 1, flag: True)", "id-a"),
                ("Probe.Rows.Mixed3(s: \"solo\", n: 2, flag: False)", "id-solo"),
            ]),
            &config(&["s", "n", "flag"], &["\"a\"".into(), "1".into(), "true".into()]),
        )
        .expect("one match");
        assert_eq!(found, "id-a");
    }
    #[test]
    fn a_qualified_enum_member_resolves_by_its_short_rendered_name() {
        let found = find_case_id(
            &cases(&[("Probe.Rows.Mixed3(c: Green)", "id-green")]),
            &config(&["c"], &["Color.Green".into()]),
        )
        .expect("a qualified enum literal matches the short-rendered case");
        assert_eq!(found, "id-green");
    }
    #[test]
    fn an_unmatchable_row_declines_naming_the_method() {
        let err = find_case_id(
            &cases(&[("Probe.Rows.Mixed3(c: Red, Green)", "id-both")]),
            &config(&["c"], &["Color.Red | Color.Green".into()]),
        )
        .expect_err("a flag combination is no enumerated row");
        assert!(err.contains("no xunit v3 test case matches"), "got: {err}");
        assert!(err.contains("Probe.Rows"), "names the method: {err}");
    }
    #[test]
    fn an_ambiguous_row_declines_instead_of_choosing() {
        let err = find_case_id(
            &cases(&[("Probe.Rows.Mixed3(s: \"a\", n: 1, flag: True)", "x"), ("Probe.Rows.Mixed3(s: \"a\", n: 1, flag: True)", "y")]),
            &config(&["s", "n", "flag"], &["\"a\"".into(), "1".into(), "true".into()]),
        )
        .expect_err("two matches is ambiguity, not a selection");
        assert!(err.contains("2 xunit v3 test cases match"), "got: {err}");
    }
}
