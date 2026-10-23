//! 入れ子 heredoc の本家記録を、差分ゲートと実ファイルの CLI 回帰で共有する。

use serde_json::Value;

use super::annotation::Annotation;
use super::case::CopCase;
use sonicop::engine::CorrectMode;

pub fn fixture() -> Value {
    serde_json::from_str(include_str!("../fixtures/nested_heredoc_divergences.json")).unwrap()
}

pub fn record<'a>(
    fixture: &'a Value,
    case: &Value,
    engine: &str,
    ending: &str,
    mode: &str,
) -> &'a Value {
    recorded(fixture, case, "reference", engine, ending, mode)
}

pub fn variable_record<'a>(
    fixture: &'a Value,
    case: &Value,
    engine: &str,
    ending: &str,
    mode: &str,
) -> &'a Value {
    recorded(fixture, case, "variable_reference", engine, ending, mode)
}

fn recorded<'a>(
    fixture: &'a Value,
    case: &Value,
    scope: &str,
    engine: &str,
    ending: &str,
    mode: &str,
) -> &'a Value {
    let key = case[scope][engine][ending][mode].as_str().unwrap();
    &fixture["records"][key]
}

/// ケースは whitequark の実診断を持ち、移植版の意図的な差はマニフェストへ置く。
/// CLI は同じ fixture の Prism 実記録で、物理 Ruby の読み方と原本バイトを検査する。
pub fn conformance_cases() -> Vec<CopCase> {
    let fixture = fixture();
    let profile = fixture["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|profile| profile["target"] == "2.7" && profile["engine"] == "parser_whitequark")
        .unwrap();
    let cops: Vec<_> = fixture["only"].as_str().unwrap().split(',').collect();
    let mut cases = Vec::new();
    for item in fixture["cases"].as_array().unwrap() {
        let source = item["source"].as_str().unwrap();
        let reference = record(&fixture, item, "whitequark", "lf", "inspect");
        for cop in &cops {
            let reference = if *cop == "Lint/UselessAssignment" && item["id"] == "used_assignment" {
                variable_record(&fixture, item, "whitequark", "lf", "inspect")
            } else {
                reference
            };
            let annotations: Vec<_> = reference["offenses"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|offense| offense["cop_name"] == *cop)
                .map(|offense| {
                    let location = &offense["location"];
                    let line = location["start_line"].as_u64().unwrap() as usize;
                    let column = location["start_column"].as_u64().unwrap() as usize;
                    let length = if location["start_line"] == location["last_line"] {
                        location["last_column"].as_u64().unwrap() as usize + 1 - column
                    } else {
                        source.lines().nth(line - 1).unwrap().chars().count() + 1 - column
                    };
                    Annotation::new(line, column, length, offense["message"].as_str().unwrap())
                })
                .collect();
            let id = format!(
                "nested_heredoc_{}_{}",
                item["id"].as_str().unwrap(),
                cop.replace('/', "_").to_ascii_lowercase()
            );
            let case = CopCase::new(cop, source, annotations)
                .id(&id)
                .target_ruby("2.7")
                .config(profile["yaml"].as_str().unwrap());
            if *cop == "Lint/UselessAssignment" {
                for (mode, correcting) in [("-a", CorrectMode::Safe), ("-A", CorrectMode::All)] {
                    let corrected = if item["id"] == "used_assignment" {
                        variable_record(&fixture, item, "whitequark", "lf", mode)
                    } else {
                        record(&fixture, item, "whitequark", "lf", mode)
                    };
                    cases.push(
                        case.clone()
                            .id(&format!(
                                "{id}_{}",
                                if mode == "-a" { "safe" } else { "unsafe" }
                            ))
                            .correct_mode(correcting)
                            .corrected_verbatim(corrected["after"].as_str().unwrap()),
                    );
                }
            } else {
                cases.push(case);
            }
        }
    }
    cases
}
