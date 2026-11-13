//! Architecture and serialization contract tests for `coresight-collect`.
//!
//! These prove the negative space around the parsers: they are total, their
//! output is bounded and camelCase, and the production source contains no I/O,
//! process, network, or database API and no panicking construct.

use std::path::{Path, PathBuf};

use coresight_collect::{
    parse_block_rules, parse_dns_candidates, parse_privacy_listing, parse_startup, LaunchdScope,
    RawStartupEntry,
};
use coresight_webtools::RuleSource;

fn hostile_inputs() -> Vec<String> {
    vec![
        String::new(),
        "   \n\t\r\n \r\n".to_string(),
        "\u{0}\u{0}\u{0}".to_string(),
        "x".repeat(1024 * 1024),
        format!("{}{}", "<array>".repeat(50_000), "</array>".repeat(50_000)),
        format!("{}{}", "<dict>".repeat(20_000), "</dict>".repeat(20_000)),
    ]
}

#[test]
fn every_parser_is_total_across_hostile_inputs() {
    for input in hostile_inputs() {
        let dns = parse_dns_candidates(&input);
        assert!(
            dns.candidates.len().saturating_add(dns.rejected.len()) <= 1,
            "one physical line can produce at most one DNS outcome"
        );

        let block = parse_block_rules(&input, RuleSource::UserDefined);
        assert!(block.rejected.len() <= 1);

        let privacy = parse_privacy_listing(&input);
        assert!(privacy.rejected.len() <= 1);

        let startup = parse_startup(&[
            RawStartupEntry::LaunchdPlist {
                label: input.clone(),
                plist_text: input.clone(),
                scope: LaunchdScope::UserAgent,
            },
            RawStartupEntry::RegistryRunValue {
                hive: input.clone(),
                key_path: input.clone(),
                value_name: input.clone(),
                command: input.clone(),
            },
            RawStartupEntry::ShellProfileLine {
                profile: input.clone(),
                line: input.clone(),
            },
            RawStartupEntry::SystemdUnit {
                unit_name: input.clone(),
                unit_text: input.clone(),
            },
        ]);
        assert_eq!(
            startup.items.len().saturating_add(startup.rejected.len()),
            4,
            "every supplied startup entry must be an item or a counted reject"
        );
    }
}

fn source_files() -> Vec<PathBuf> {
    fn visit(directory: &Path, files: &mut Vec<PathBuf>) {
        let entries = match std::fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                visit(&path, files);
            } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
                files.push(path);
            }
        }
    }

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    visit(&root, &mut files);
    files
}

#[test]
fn production_source_has_no_io_process_network_or_database_apis() {
    for path in source_files() {
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(_) => continue,
        };
        for forbidden in [
            "std::fs::",
            "std::process",
            "std::net",
            "std::os::windows",
            "std::os::unix",
            "rusqlite",
            "Command::new",
            "reqwest",
            "tokio",
            "sqlx",
        ] {
            assert!(
                !text.contains(forbidden),
                "{} contains forbidden API {forbidden}",
                path.display()
            );
        }
    }
}

#[test]
fn production_source_has_no_panicking_constructs() {
    for path in source_files() {
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(_) => continue,
        };
        for forbidden in [
            "unwrap()",
            "expect(",
            "panic!(",
            "todo!(",
            "unimplemented!(",
            "unreachable!(",
        ] {
            assert!(
                !text.contains(forbidden),
                "{} contains panicking construct {forbidden}",
                path.display()
            );
        }
    }
}

#[test]
fn rejected_lines_and_sources_are_bounded() {
    let junk = "x".repeat(200_000);
    let dns = parse_dns_candidates(&junk);
    assert_eq!(dns.rejected.len(), 1);
    assert!(dns.rejected[0].text.chars().count() <= 200);

    let startup = parse_startup(&[RawStartupEntry::LaunchdPlist {
        label: junk.clone(),
        plist_text: junk.clone(),
        scope: LaunchdScope::UserAgent,
    }]);
    assert_eq!(startup.rejected.len(), 1);
    assert!(
        startup.rejected[0].source.chars().count()
            <= coresight_collect::startup::MAX_REJECT_SOURCE_CHARS
    );
}

#[test]
fn results_serialize_with_camel_case_keys_and_screaming_enums() {
    let startup = parse_startup(&[RawStartupEntry::ShellProfileLine {
        profile: "~/.profile".to_string(),
        line: "helper --flag".to_string(),
    }]);
    let startup_json = serde_json::to_value(&startup).unwrap();
    assert!(startup_json.get("items").is_some());
    assert!(startup_json.get("rejected").is_some());
    assert!(startup_json.get("unitParseFailures").is_some());
    assert_eq!(
        startup_json["items"][0]["source"]["UNKNOWN"]["raw"],
        "shell profile ~/.profile: helper --flag"
    );

    let raw_entry = serde_json::to_value(RawStartupEntry::LaunchdPlist {
        label: "label".to_string(),
        plist_text: "text".to_string(),
        scope: LaunchdScope::SystemDaemon,
    })
    .unwrap();
    assert!(raw_entry.get("LAUNCHD_PLIST").is_some());
    assert_eq!(raw_entry["LAUNCHD_PLIST"]["plistText"], "text");
    assert_eq!(raw_entry["LAUNCHD_PLIST"]["scope"], "SYSTEM_DAEMON");

    let dns = parse_dns_candidates(
        "Example | ipv4=192.0.2.1 | dnssec=yes | filter=no | privacy=unknown\n",
    );
    let dns_json = serde_json::to_value(&dns).unwrap();
    assert!(dns_json.get("candidates").is_some());
    assert!(dns_json.get("rejected").is_some());
    assert_eq!(dns_json["candidates"][0]["supportsDnssec"], true);
    assert_eq!(dns_json["candidates"][0]["privacyPolicy"], "UNKNOWN");

    let block = parse_block_rules("example.com,advertising\n", RuleSource::UserDefined);
    let block_json = serde_json::to_value(&block).unwrap();
    assert!(block_json.get("rules").is_some());
    assert!(block_json.get("plan").is_some());
    assert_eq!(block_json["defaultedCategories"], 0);

    let privacy = parse_privacy_listing("cache_entry,1,unknown\n");
    let privacy_json = serde_json::to_value(&privacy).unwrap();
    assert_eq!(privacy_json["findings"][0]["kind"], "CACHE_ENTRY");
    assert_eq!(
        privacy_json["findings"][0]["estimatedBytes"],
        serde_json::Value::Null
    );
}
