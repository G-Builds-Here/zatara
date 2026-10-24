//! xunit v3 / MTP single-case selection (XUNIT-V3-MTP).
//!
//! v3 has no argument-level filter: the runner's only way to run ONE test case
//! is `-id <unique id>`, and those ids are opaque content-hashes that only
//! `-list` discovery yields. So a gutter row click on a v3 `[Theory]` resolves
//! through three steps, all of them this module:
//!
//! 1. enumerate — run the built test dll with `-preEnumerateTheories -list
//!    full/json` and read every case's `DisplayName` + `ID` (pre-enumeration is
//!    what turns each `[InlineData]` row into its own addressable case; without
//!    it a theory is one node and the row ids do not exist).
//! 2. match — rebuild the clicked row's display name from the captures the
//!    gutter already sends (method FQN + parameter names + RAW argument tokens)
//!    and find the enumerated case with that exact display name.
//! 3. run — `-preEnumerateTheories -id <id>` (the flag rides the run too, or the
//!    id names nothing and the runner selects zero).
//!
//! Decline-don't-guess, the same law as the Run CLI: zero or many matches is an
//! honest failure naming the method, never a wrong-scope run of the whole suite.
//!
//! The display-name formatting was MEASURED on xunit.v3 3.2.2 (2026-10-24), not
//! guessed — see `normalize_arg` and the tests. Two transforms take a source
//! argument literal to xunit's rendering: bool casing (`true` to `True`) and
//! the qualified enum member (`Color.Green` renders as the short `Green`).
//! Everything else passes through — strings keep their quotes, chars keep
//! their single quotes, `null` and numbers ride on, and invariant-culture
//! doubles already read the same. Shapes beyond those (flag combinations,
//! casts, custom display names) match nothing and fall to the honest decline.

use std::path::Path;

use zatara_types::LaunchConfig;

/// One enumerated test case.
pub(crate) struct TestCase {
    pub display: String,
    pub id: String,
}

/// The argv tail (after `dotnet <dll>`) that enumerates every test case,
/// theory rows expanded into their own addressable cases.
pub(crate) fn list_tail_args() -> [&'static str; 3] {
    ["-preEnumerateTheories", "-list", "full/json"]
}

/// The argv (after `dotnet <dll>`) that runs exactly one enumerated case. The
/// pre-enumeration flag must be present here as well — a theory case's id is
/// only discovered, and therefore only matchable, under pre-enumeration.
pub(crate) fn id_args(id: &str) -> Vec<String> {
    ["-preEnumerateTheories", "-id", id]
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// One captured argument token (the RAW, as-written source literal) in xunit's
/// display formatting. Everything passes through verbatim except two forms:
/// the bool literals, which xunit title-cases, and a QUALIFIED ENUM MEMBER
/// (`Color.Green`), which xunit renders by SHORT name (`Green`) — measured
/// 2026-10-24. The enum rule takes the last `.`-segment of a bare qualified
/// identifier; any other value shaped that way (a qualified const, say)
/// renders by value and simply fails to match, landing in the honest decline
/// — the rule can never widen or misselect. Trimmed so a captured token with
/// stray spacing still matches the runner's own trimmed rendering.
pub(crate) fn normalize_arg(raw: &str) -> String {
    let trimmed = raw.trim();
    match trimmed {
        "true" => "True".to_string(),
        "false" => "False".to_string(),
        other => last_segment_if_qualified_identifier(other).to_string(),
    }
}

/// The last `.`-segment of a bare qualified identifier (`Color.Green`,
/// `Ns.Color.Green`), else the value untouched. Only identifier segments
/// qualify — quoted text, casts (`(Color)1`), flag combinations
/// (`A.Red | A.Green`), and numbers never match the shape.
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

/// The display name xunit gives the case the captures name:
/// `Namespace.Class.Method(param: value, param2: value2)` — the same `param:
/// value` token and `", "` join `runner::build_filter` already builds for the
/// vstest lane, wrapped in the method's parentheses (which a vstest filter value
/// can never carry, but a display name must).
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

/// Parse `-list full/json` stdout — a JSON array of `{DisplayName, ID, …}` —
/// into enumerated cases. The runner prints its banner before the array, so the
/// parse window is the outermost brackets. A stdout with no array parses to
/// empty (the caller then declines honestly on zero matches).
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

/// The id of the enumerated case matching the config's row, or a decline
/// message naming the method. Pure over the enumerated list so the whole
/// match-or-decline law tests without building anything.
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

/// Run the built test dll's enumeration and return every case. Uses `dotnet
/// <dll>` (measured equivalent of the apphost for `-list` and `-id`).
pub(crate) fn enumerate(dll: &Path) -> Result<Vec<TestCase>, String> {
    let out = std::process::Command::new("dotnet")
        .arg(dll)
        .args(list_tail_args())
        .output()
        .map_err(|e| format!("failed to spawn `dotnet {} -list`: {e}", dll.display()))?;
    Ok(parse_cases(&String::from_utf8_lossy(&out.stdout)))
}

/// The full row-id resolution an action needs: enumerate the built dll, then
/// match the config's row to its unique id.
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
        // Measured 2026-10-24: `true` renders `True` and `Color.Green` renders
        // `Green`; everything else verbatim (quoted strings, single-quoted
        // chars, `null`, invariant doubles).
        assert_eq!(normalize_arg("true"), "True");
        assert_eq!(normalize_arg("false"), "False");
        assert_eq!(normalize_arg("Color.Green"), "Green");
        assert_eq!(normalize_arg("Ns.Deep.Color.Red"), "Red");
        assert_eq!(normalize_arg("\"solo\""), "\"solo\"");
        assert_eq!(normalize_arg("'x'"), "'x'");
        assert_eq!(normalize_arg("null"), "null");
        assert_eq!(normalize_arg("1.5"), "1.5");
        // Only a bare qualified identifier is shortened; a cast, a flag
        // combination, and a number never match the shape.
        assert_eq!(normalize_arg("(Color)1"), "(Color)1");
        assert_eq!(normalize_arg("Color.Red | Color.Green"), "Color.Red | Color.Green");
        assert_eq!(normalize_arg("1.2.3"), "1.2.3");
    }

    #[test]
    fn the_display_name_is_the_method_fqn_plus_param_value_pairs() {
        // `s: "a", n: 1, flag: True` — the exact enumerated form, quotes and all.
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
        // A runner that errored before listing yields no cases — the caller
        // declines on zero matches rather than inventing one.
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
        // Measured 2026-10-24: source `Color.Green` renders `c: Green` — the
        // matcher shortens the qualifier (the owner live cycle first
        // declined this row, before the rule existed).
        let found = find_case_id(
            &cases(&[("Probe.Rows.Mixed3(c: Green)", "id-green")]),
            &config(&["c"], &["Color.Green".into()]),
        )
        .expect("a qualified enum literal matches the short-rendered case");
        assert_eq!(found, "id-green");
    }

    #[test]
    fn an_unmatchable_row_declines_naming_the_method() {
        // Beyond bool casing and the enum short name nothing is transformed:
        // a flag combination is not a bare member, so the exact match fails
        // and the run declines instead of guessing a scope. The decline
        // points at the method click that always works.
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
        // Duplicate display names (only possible with a custom display-name
        // scheme) must not be broken by picking one — declining is the honest
        // answer, the whole method is the safe scope.
        let err = find_case_id(
            &cases(&[("Probe.Rows.Mixed3(s: \"a\", n: 1, flag: True)", "x"), ("Probe.Rows.Mixed3(s: \"a\", n: 1, flag: True)", "y")]),
            &config(&["s", "n", "flag"], &["\"a\"".into(), "1".into(), "true".into()]),
        )
        .expect_err("two matches is ambiguity, not a selection");
        assert!(err.contains("2 xunit v3 test cases match"), "got: {err}");
    }
}
