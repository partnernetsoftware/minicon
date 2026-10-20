//! Return-value oracle independent of the native client and host implementation.
pub fn kinds(system: bool) -> Vec<String> {
    let mut result = Vec::new();
    for api in ["wide", "file", "mixed", "utf8"] {
        if system && api == "utf8" {
            continue;
        }
        for lifetime in ["share", "flush", "close"] {
            result.push(format!("{api}-{lifetime}"));
        }
    }
    result
}
pub fn expected(kind: &str) -> String {
    let (api, lifetime) = kind.split_once('-').unwrap();
    let first = if api == "utf8" { vec![228] } else { vec![97] };
    let rest = if lifetime == "close" {
        vec![]
    } else if api == "utf8" {
        if lifetime == "flush" {
            vec![195, 169, 13, 10]
        } else {
            vec![184, 173, 195, 169, 13, 10]
        }
    } else {
        vec![98, 99, 13, 10]
    };
    format!(
        "first={first:?};other=[90];rest={rest:?};after=[88];flush_wait={};final_wait=258",
        if lifetime == "flush" { 258 } else { 0 }
    )
}
